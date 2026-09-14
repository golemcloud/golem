// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

tokio::task_local! {
    pub(super) static MUTATION_SCOPE: Arc<ProducerMutationScope>;
    static MUTATION_EFFECTS: ProducerMutationEffects;
}

pub(super) struct ProducerMutationScope {
    pub(super) producer: Arc<DurableStreamProducer>,
    commit_tails: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub(super) session_records_changed: AtomicBool,
    pub(super) publications: std::sync::Mutex<Vec<PublicationReceipt>>,
    remote_cancellations: std::sync::Mutex<
        Vec<futures::future::BoxFuture<'static, Result<(), DurableStreamProducerError>>>,
    >,
    _operation: OwnedSemaphorePermit,
    _memory: OwnedSemaphorePermit,
}

struct ProducerMutationEffects {
    producer: Arc<DurableStreamProducer>,
    pending: AtomicBool,
}

impl Drop for ProducerMutationEffects {
    fn drop(&mut self) {
        if self.pending.load(Ordering::Acquire) {
            self.producer.poison();
        }
    }
}

impl DurableStreamProducer {
    pub(crate) fn ensure_healthy(&self) -> Result<(), DurableStreamProducerError> {
        if self.poisoned.load(Ordering::Acquire) {
            Err(DurableStreamProducerError::RecoveryRequired)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn wait_durable_drained(&self) {
        self.durable_activity.wait_drained().await;
    }

    pub(crate) async fn with_metadata_activity<T>(
        &self,
        lookup: impl Future<Output = T>,
    ) -> Result<T, DurableStreamProducerError> {
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        let result = activity.scope(lookup).await;
        self.ensure_healthy()?;
        Ok(result)
    }

    /// Cancels only a remote waiter; local durable mutations must finish independently.
    pub(crate) async fn remote_until_retired<T>(
        &self,
        remote: impl Future<Output = Result<T, DurableStreamProducerError>>,
    ) -> Result<T, DurableStreamProducerError> {
        tokio::select! {
            biased;
            _ = self.retirement.cancelled() => Err(DurableStreamProducerError::RecoveryRequired),
            result = remote => result,
        }
    }

    pub(crate) fn try_retire_quiescent(&self) -> bool {
        if self.ensure_healthy().is_err() || !self.durable_activity.try_close_if_idle() {
            return false;
        }
        self.poison();
        true
    }

    pub(crate) fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.retirement.cancel();
        self.durable_activity.close();
        self.owned_operations.close();
        self.owned_operation_bytes.close();
        self.lifecycle_operations.close();
        self.lifecycle_operation_bytes.close();
        for bus in self
            .buses
            .read()
            .expect("durable stream bus map lock poisoned")
            .values()
        {
            bus.retire();
        }
        self.terminal_progress.notify_one();
        self.session_records_changed.notify_waiters();
    }

    pub(super) fn begin_durable_effect(&self) {
        let _ = MUTATION_EFFECTS.try_with(|scope| {
            if std::ptr::eq(scope.producer.as_ref(), self) {
                scope.pending.store(true, Ordering::Release);
            }
        });
    }

    pub(super) fn finish_durable_effect(&self) {
        let _ = MUTATION_EFFECTS.try_with(|scope| {
            if std::ptr::eq(scope.producer.as_ref(), self) {
                scope.pending.store(false, Ordering::Release);
            }
        });
    }

    fn track_durable_effects<T, E>(
        self: &Arc<Self>,
        operation: impl Future<Output = Result<T, E>>,
    ) -> impl Future<Output = Result<T, E>> {
        let operation = Box::pin(operation);
        MUTATION_EFFECTS.scope(
            ProducerMutationEffects {
                producer: self.clone(),
                pending: AtomicBool::new(false),
            },
            async move {
                let outcome = operation.await;
                if outcome.is_ok() {
                    self.finish_durable_effect();
                }
                outcome
            },
        )
    }

    pub(crate) async fn run_owned<T, E, F, Fut>(
        &self,
        retained_bytes: usize,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DurableStreamProducerError> + Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.run_owned_mutation(retained_bytes, false, operation)
            .await
    }

    pub(crate) async fn run_lifecycle<T, E, F, Fut>(
        &self,
        retained_bytes: usize,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DurableStreamProducerError> + Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.run_owned_mutation(retained_bytes, true, operation)
            .await
    }

    pub(crate) fn defer_remote_cancellation(
        &self,
        cancellation: impl Future<Output = Result<(), DurableStreamProducerError>> + Send + 'static,
    ) {
        MUTATION_SCOPE.with(|scope| {
            assert!(std::ptr::eq(scope.producer.as_ref(), self));
            scope
                .remote_cancellations
                .lock()
                .expect("remote cancellation list lock poisoned")
                .push(Box::pin(cancellation));
        });
    }

    async fn run_owned_mutation<T, E, F, Fut>(
        &self,
        retained_bytes: usize,
        lifecycle: bool,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DurableStreamProducerError> + Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.ensure_healthy()?;
        let producer = self
            .self_weak
            .upgrade()
            .expect("live producer has an owning Arc");
        if MUTATION_SCOPE
            .try_with(|scope| Arc::ptr_eq(&scope.producer, &producer))
            .unwrap_or(false)
        {
            return producer
                .track_durable_effects(operation(producer.clone()))
                .await;
        }
        if !lifecycle && retained_bytes > 256 * 1024 * 1024 {
            return Err(DurableStreamProducerError::ItemTooLarge.into());
        }
        let (operations, bytes) = if lifecycle {
            (&self.lifecycle_operations, &self.lifecycle_operation_bytes)
        } else {
            (&self.owned_operations, &self.owned_operation_bytes)
        };
        let permit = operations
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DurableStreamProducerError::RecoveryRequired)?;
        let memory = bytes
            .clone()
            .acquire_many_owned(
                // A single large lifecycle error must still be finalized. Reserving the
                // entire lane excludes other byte-retaining lifecycle operations.
                retained_bytes.min(256 * 1024 * 1024) as u32,
            )
            .await
            .map_err(|_| DurableStreamProducerError::RecoveryRequired)?;
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .try_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        let (reply, result) = oneshot::channel();
        let scope = Arc::new(ProducerMutationScope {
            producer: producer.clone(),
            commit_tails: std::sync::Mutex::new(Vec::new()),
            session_records_changed: AtomicBool::new(false),
            publications: std::sync::Mutex::new(Vec::new()),
            remote_cancellations: std::sync::Mutex::new(Vec::new()),
            _operation: permit,
            _memory: memory,
        });
        tokio::spawn(async move {
            MUTATION_SCOPE
                .scope(scope.clone(), async move {
                    let mut outcome = match std::panic::AssertUnwindSafe(async {
                        activity
                            .clone()
                            .scope(producer.track_durable_effects(operation(producer.clone())))
                            .await
                    })
                    .catch_unwind()
                    .await
                    {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            producer.poison();
                            Err(DurableStreamProducerError::Oplog(
                                "durable stream mutation panicked".into(),
                            )
                            .into())
                        }
                    };
                    let mut publications = std::mem::take(
                        &mut *scope
                            .publications
                            .lock()
                            .expect("publication receipt list lock poisoned"),
                    );
                    let tails = std::mem::take(
                        &mut *scope
                            .commit_tails
                            .lock()
                            .expect("commit tail list lock poisoned"),
                    );
                    for tail in tails {
                        if let Err(error) = tail.await {
                            producer.poison();
                            outcome = Err(DurableStreamProducerError::Oplog(format!(
                                "durable stream commit callback failed: {error}"
                            ))
                            .into());
                        }
                    }
                    // Slot readers use published worker status, which is folded after the
                    // durability receipt but before the commit callback completes.
                    if scope.session_records_changed.load(Ordering::Acquire) {
                        producer.session_records_changed.notify_waiters();
                    }
                    // Storage quiescence excludes live fanout. The separate count/byte
                    // reservations still bound normal publications until delivery completes.
                    drop(activity);
                    let cancellations = std::mem::take(
                        &mut *scope
                            .remote_cancellations
                            .lock()
                            .expect("remote cancellation list lock poisoned"),
                    );
                    if outcome.is_ok() {
                        for cancellation in cancellations {
                            // A routed call must not inherit local mutation admission or
                            // durable activity, even when it routes back to this executor.
                            match tokio::spawn(cancellation).await {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => outcome = Err(error.into()),
                                Err(error) => {
                                    outcome = Err(DurableStreamProducerError::Oplog(format!(
                                        "remote stream cancellation failed: {error}"
                                    ))
                                    .into());
                                }
                            }
                        }
                    }
                    if !lifecycle {
                        for publication in publications.drain(..) {
                            if let Err(error) = publication
                                .await
                                .unwrap_or(Err(DurableLiveStreamBusError::PublicationAborted))
                            {
                                producer.poison();
                                outcome = Err(DurableStreamProducerError::from(error).into());
                            }
                        }
                    }
                    let _ = reply.send((outcome, publications));
                })
                .await;
        });
        let (mut outcome, publications) = result
            .await
            .expect("durable stream producer-owned operation terminated");
        // Terminal delivery is owned by the shared dispatcher. The request still observes
        // backpressure, but neither a stalled nor an abandoned request holds lifecycle admission.
        for publication in publications {
            if let Err(error) = publication
                .await
                .unwrap_or(Err(DurableLiveStreamBusError::PublicationAborted))
            {
                self.poison();
                outcome = Err(DurableStreamProducerError::from(error).into());
            }
        }
        outcome
    }

    pub(super) async fn commit(&self) {
        self.begin_durable_effect();
        let scope = MUTATION_SCOPE
            .try_with(Arc::clone)
            .ok()
            .filter(|scope| std::ptr::eq(scope.producer.as_ref(), self));
        let Some(scope) = scope else {
            (self.commit)(None).await;
            return;
        };
        let (committed, receipt) = oneshot::channel();
        let callback = (self.commit)(Some(committed));
        let producer = scope.producer.clone();
        let task = spawn_with_activity(async move {
            if let Err(panic) = std::panic::AssertUnwindSafe(callback).catch_unwind().await {
                producer.poison();
                std::panic::resume_unwind(panic);
            }
        });
        scope
            .commit_tails
            .lock()
            .expect("commit tail list lock poisoned")
            .push(task);
        receipt
            .await
            .expect("durable stream commit failed before durability receipt");
    }

    pub(super) async fn commit_notifying(&self, committed: oneshot::Sender<()>) {
        self.commit().await;
        let _ = committed.send(());
    }
}
