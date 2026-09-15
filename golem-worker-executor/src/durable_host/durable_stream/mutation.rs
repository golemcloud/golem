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
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use tokio::sync::mpsc;

type Mutation = BoxFuture<'static, MutationCompletion>;

struct MutationCompletion {
    finish: BoxFuture<'static, ()>,
}

pub(super) struct MutationQueue(std::sync::Mutex<Option<mpsc::UnboundedSender<Mutation>>>);

impl MutationQueue {
    pub(super) fn new() -> Self {
        // Admission bounds both queued mutations and their outstanding delivery work.
        let (sender, mut receiver) = mpsc::unbounded_channel::<Mutation>();
        tokio::spawn(async move {
            let mut current: Option<Mutation> = None;
            let mut completions = FuturesUnordered::new();
            let mut closed = false;
            loop {
                tokio::select! {
                    job = receiver.recv(), if !closed && current.is_none() => {
                        match job {
                            Some(job) => current = Some(job),
                            None => closed = true,
                        }
                    }
                    completion = async { current.as_mut().unwrap().await }, if current.is_some() => {
                        current = None;
                        completions.push(completion.finish);
                    }
                    Some(()) = completions.next(), if !completions.is_empty() => {}
                    else => break,
                }
            }
        });
        Self(std::sync::Mutex::new(Some(sender)))
    }

    fn send(&self, mutation: Mutation) -> Result<(), StreamStoreError> {
        let sender = self.0.lock().unwrap();
        sender
            .as_ref()
            .ok_or(StreamStoreError::RecoveryRequired)?
            .send(mutation)
            .map_err(|_| StreamStoreError::RecoveryRequired)
    }

    fn close(&self) {
        self.0.lock().unwrap().take();
    }
}

struct ProducerMutationScope {
    producer: Arc<DurableStreamStore>,
    commit_tails: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    session_records_changed: AtomicBool,
    publications: std::sync::Mutex<Vec<PublicationReceipt>>,
    remote_cancellations:
        std::sync::Mutex<Vec<futures::future::BoxFuture<'static, Result<(), StreamStoreError>>>>,
    _operation: OwnedSemaphorePermit,
    _memory: OwnedSemaphorePermit,
}

/// An admitted write's effects and completion obligations, passed explicitly to nested writes.
pub(crate) struct StreamWriteContext {
    scope: Arc<ProducerMutationScope>,
    effects: Arc<WriteEffects>,
}

struct WriteEffects {
    pending: AtomicBool,
    active: AtomicBool,
}

struct ProducerMutationEffects {
    producer: Arc<DurableStreamStore>,
    effects: Arc<WriteEffects>,
}

impl Drop for ProducerMutationEffects {
    fn drop(&mut self) {
        self.effects.active.store(false, Ordering::Release);
        if self.effects.pending.load(Ordering::Acquire) {
            self.producer.poison();
        }
    }
}

impl StreamWriteContext {
    async fn execute<T, E, F, Fut>(scope: Arc<ProducerMutationScope>, operation: F) -> Result<T, E>
    where
        F: FnOnce(Arc<DurableStreamStore>, Self) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let effects = Arc::new(WriteEffects {
            pending: AtomicBool::new(false),
            active: AtomicBool::new(true),
        });
        let guard = ProducerMutationEffects {
            producer: scope.producer.clone(),
            effects: effects.clone(),
        };
        let context = Self {
            scope: scope.clone(),
            effects,
        };
        let outcome = operation(scope.producer.clone(), context).await;
        if outcome.is_ok() {
            guard.effects.pending.store(false, Ordering::Release);
        }
        drop(guard);
        outcome
    }

    /// Runs a nested write inline under the existing admission, with independent failure tracking.
    pub(crate) async fn run_nested<T, E, F, Fut>(&self, operation: F) -> Result<T, E>
    where
        E: From<StreamStoreError>,
        F: FnOnce(Arc<DurableStreamStore>, Self) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.assert_owner(&self.scope.producer);
        self.scope.producer.ensure_healthy()?;
        Self::execute(self.scope.clone(), operation).await
    }

    /// Rejects use by another store or after this write has returned or been dropped.
    pub(crate) fn assert_owner(&self, store: &DurableStreamStore) {
        assert!(std::ptr::eq(self.scope.producer.as_ref(), store));
        assert!(
            self.effects.active.load(Ordering::Acquire),
            "stream write context used after its operation completed"
        );
    }

    /// Marks a durable effect that must finish before this operation can fail safely.
    pub(crate) fn begin_durable_effect(&self) {
        self.assert_owner(&self.scope.producer);
        self.effects.pending.store(true, Ordering::Release);
    }

    /// Marks this operation's effects complete without clearing a parent's or sibling's effects.
    pub(crate) fn finish_durable_effect(&self) {
        self.assert_owner(&self.scope.producer);
        self.effects.pending.store(false, Ordering::Release);
    }

    /// Defers cancellation until local durability and status callbacks have completed.
    pub(crate) fn defer_remote_cancellation(
        &self,
        cancellation: impl Future<Output = Result<(), StreamStoreError>> + Send + 'static,
    ) {
        self.assert_owner(&self.scope.producer);
        self.scope
            .remote_cancellations
            .lock()
            .expect("remote cancellation list lock poisoned")
            .push(Box::pin(cancellation));
    }

    /// Defers reader notifications until the committed session status has been published.
    pub(crate) fn notify_session_records_changed(&self) {
        self.assert_owner(&self.scope.producer);
        self.scope
            .session_records_changed
            .store(true, Ordering::Release);
    }

    /// Keeps admission charged while the live bus retains this write's payloads.
    pub(super) fn publication_keepalive(&self) -> Arc<dyn Send + Sync> {
        self.assert_owner(&self.scope.producer);
        self.scope.clone()
    }

    /// Defers delivery backpressure until the serial write body has released the queue.
    pub(crate) fn defer_publication(&self, publication: PublicationReceipt) {
        self.assert_owner(&self.scope.producer);
        self.scope
            .publications
            .lock()
            .expect("publication receipt list lock poisoned")
            .push(publication);
    }
}

impl DurableStreamStore {
    /// Rejects work after the resident producer has been poisoned or retired.
    pub(crate) fn ensure_healthy(&self) -> Result<(), StreamStoreError> {
        if self.poisoned.load(Ordering::Acquire) {
            Err(StreamStoreError::RecoveryRequired)
        } else {
            Ok(())
        }
    }

    /// Waits until every admitted durable effect has completed.
    pub(crate) async fn wait_durable_drained(&self) {
        self.durable_activity.wait_drained().await;
    }

    /// Keeps retirement from completing while metadata is being read or projected.
    pub(crate) async fn with_metadata_activity<T>(
        &self,
        lookup: impl Future<Output = T>,
    ) -> Result<T, StreamStoreError> {
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(StreamStoreError::RecoveryRequired)?;
        let result = activity.scope(lookup).await;
        self.ensure_healthy()?;
        Ok(result)
    }

    /// Cancels only the forwarded waiter on retirement; local durable mutations finish independently.
    pub(crate) async fn remote_until_retired<T>(
        &self,
        remote: impl Future<Output = Result<T, StreamStoreError>>,
    ) -> Result<T, StreamStoreError> {
        tokio::select! {
            biased;
            _ = self.retirement.cancelled() => Err(StreamStoreError::RecoveryRequired),
            result = remote => result,
        }
    }

    /// Retires only when no durable or metadata work remains active.
    pub(crate) fn try_retire_quiescent(&self) -> bool {
        if self.ensure_healthy().is_err() || !self.durable_activity.try_close_if_idle() {
            return false;
        }
        self.poison();
        true
    }

    /// Prevents new work and cancels resident forwarded operations after failure or retirement.
    pub(crate) fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.retirement.cancel();
        self.durable_activity.close();
        self.mutations.close();
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

    /// Queues one producer mutation and resolves after its durable callback completes.
    pub(crate) async fn run_owned<T, E, F, Fut>(
        &self,
        context: Option<&StreamWriteContext>,
        retained_bytes: usize,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StreamStoreError> + Send + 'static,
        F: FnOnce(Arc<Self>, StreamWriteContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.run_owned_mutation(context, retained_bytes, false, operation)
            .await
    }

    /// Queues lifecycle work separately from ordinary producer mutations.
    pub(crate) async fn run_lifecycle<T, E, F, Fut>(
        &self,
        context: Option<&StreamWriteContext>,
        retained_bytes: usize,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StreamStoreError> + Send + 'static,
        F: FnOnce(Arc<Self>, StreamWriteContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.run_owned_mutation(context, retained_bytes, true, operation)
            .await
    }

    async fn run_owned_mutation<T, E, F, Fut>(
        &self,
        context: Option<&StreamWriteContext>,
        retained_bytes: usize,
        lifecycle: bool,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StreamStoreError> + Send + 'static,
        F: FnOnce(Arc<Self>, StreamWriteContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.ensure_healthy()?;
        if let Some(context) = context {
            context.assert_owner(self);
            return context.run_nested(operation).await;
        }
        let producer = self
            .self_weak
            .upgrade()
            .expect("live producer has an owning Arc");
        if !lifecycle && retained_bytes > 256 * 1024 * 1024 {
            return Err(StreamStoreError::ItemTooLarge.into());
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
            .map_err(|_| StreamStoreError::RecoveryRequired)?;
        let memory = bytes
            .clone()
            .acquire_many_owned(
                // A single large lifecycle error must still be finalized. Reserving the
                // entire lane excludes other byte-retaining lifecycle operations.
                retained_bytes.min(256 * 1024 * 1024) as u32,
            )
            .await
            .map_err(|_| StreamStoreError::RecoveryRequired)?;
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .try_enter()
            .ok_or(StreamStoreError::RecoveryRequired)?;
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
        self.mutations.send(Box::pin(async move {
            let mut outcome = match std::panic::AssertUnwindSafe(async {
                producer.ensure_healthy()?;
                activity
                    .clone()
                    .scope(StreamWriteContext::execute(scope.clone(), operation))
                    .await
            })
            .catch_unwind()
            .await
            {
                Ok(outcome) => outcome,
                Err(_) => {
                    producer.poison();
                    Err(StreamStoreError::Oplog("durable stream mutation panicked".into()).into())
                }
            };
            // Callback tails, remote calls and live fanout must not block the
            // next mutation or reuse its write context. The queue drives
            // these completions alongside the next durable mutation.
            MutationCompletion {
                finish: async move {
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
                            outcome = Err(StreamStoreError::Oplog(format!(
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
                    // Draining storage work excludes live fanout. The separate count/byte
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
                            match std::panic::AssertUnwindSafe(cancellation)
                                .catch_unwind()
                                .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => outcome = Err(error.into()),
                                Err(_) => {
                                    outcome = Err(StreamStoreError::Oplog(
                                        "remote stream cancellation panicked".into(),
                                    )
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
                                outcome = Err(StreamStoreError::from(error).into());
                            }
                        }
                    }
                    let _ = reply.send((outcome, publications));
                }
                .boxed(),
            }
        }))?;
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
                outcome = Err(StreamStoreError::from(error).into());
            }
        }
        outcome
    }

    pub(super) async fn commit(&self, context: &StreamWriteContext) {
        context.assert_owner(self);
        context.begin_durable_effect();
        let scope = &context.scope;
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

    pub(super) async fn commit_notifying(
        &self,
        context: &StreamWriteContext,
        committed: oneshot::Sender<()>,
    ) {
        self.commit(context).await;
        let _ = committed.send(());
    }
}
