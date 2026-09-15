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

enum MutationJob {
    Write(Mutation),
    Detached(BoxFuture<'static, ()>),
}

pub(super) struct MutationQueue(std::sync::Mutex<Option<mpsc::UnboundedSender<MutationJob>>>);

impl MutationQueue {
    pub(super) fn new() -> Self {
        // Admission bounds both queued mutations and their outstanding delivery work.
        let (sender, mut receiver) = mpsc::unbounded_channel::<MutationJob>();
        tokio::spawn(async move {
            let mut current: Option<Mutation> = None;
            let mut pending = std::collections::VecDeque::new();
            let mut completions = FuturesUnordered::new();
            let mut closed = false;
            loop {
                if current.is_none() {
                    current = pending.pop_front();
                }
                tokio::select! {
                    job = receiver.recv(), if !closed => {
                        match job {
                            Some(MutationJob::Write(job)) => pending.push_back(job),
                            Some(MutationJob::Detached(job)) => completions.push(job),
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

    fn send(&self, job: MutationJob) -> Result<(), StreamStoreError> {
        let sender = self.0.lock().unwrap();
        sender
            .as_ref()
            .ok_or(StreamStoreError::RecoveryRequired)?
            .send(job)
            .map_err(|_| StreamStoreError::RecoveryRequired)
    }

    fn close(&self) {
        self.0.lock().unwrap().take();
    }
}

struct ProducerMutationScope {
    producer: Arc<DurableStreamStore>,
    admission: Arc<StreamWriteAdmission>,
    commit_tails: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    session_records_changed: AtomicBool,
}

/// Bounds a detached operation across local writes and remote waits without readmission.
pub(crate) struct StreamWriteAdmission {
    producer: Arc<DurableStreamStore>,
    status_receipts: std::sync::Mutex<Vec<oneshot::Receiver<Result<(), StreamStoreError>>>>,
    publications: std::sync::Mutex<Vec<PublicationReceipt>>,
    _operation: OwnedSemaphorePermit,
    _memory: OwnedSemaphorePermit,
}

impl StreamWriteAdmission {
    /// Queues a local write and waits for durability; the admitted operation joins status callbacks.
    pub(crate) async fn submit<T, E, F, Fut>(self: &Arc<Self>, operation: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StreamStoreError> + Send + 'static,
        F: FnOnce(Arc<DurableStreamStore>, StreamWriteContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.producer.submit(self.clone(), operation).await
    }

    /// Defers live delivery until the operation has released its session lock.
    pub(crate) fn defer_publication(&self, publication: PublicationReceipt) {
        self.publications
            .lock()
            .expect("publication receipt list lock poisoned")
            .push(publication);
    }
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
        self.scope.admission.clone()
    }

    /// Defers delivery backpressure until the serial write body has released the queue.
    pub(crate) fn defer_publication(&self, publication: PublicationReceipt) {
        self.assert_owner(&self.scope.producer);
        self.scope.admission.defer_publication(publication);
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
        let result = tokio::select! {
            biased;
            _ = self.retirement.cancelled() => Err(StreamStoreError::RecoveryRequired),
            result = remote => result,
        }?;
        self.ensure_healthy()?;
        Ok(result)
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
        self.run_admitted(
            None,
            retained_bytes,
            lifecycle,
            move |_, admission| async move { admission.submit(operation).await },
        )
        .await
    }

    /// Runs detached orchestration under one reservation, outside the serial local writer.
    /// Reusing an admission avoids acquiring a second count or byte reservation for nested work.
    pub(crate) async fn run_admitted<T, E, F, Fut>(
        &self,
        admission: Option<&Arc<StreamWriteAdmission>>,
        retained_bytes: usize,
        lifecycle: bool,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StreamStoreError> + Send + 'static,
        F: FnOnce(Arc<Self>, Arc<StreamWriteAdmission>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.ensure_healthy()?;
        if let Some(admission) = admission {
            assert!(std::ptr::eq(admission.producer.as_ref(), self));
            return operation(admission.producer.clone(), admission.clone()).await;
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
        let admission = Arc::new(StreamWriteAdmission {
            producer: producer.clone(),
            status_receipts: std::sync::Mutex::new(Vec::new()),
            publications: std::sync::Mutex::new(Vec::new()),
            _operation: permit,
            _memory: memory,
        });
        let (reply, result) = oneshot::channel();
        self.mutations
            .send(MutationJob::Detached(Box::pin(async move {
                let mut outcome = match std::panic::AssertUnwindSafe(async {
                    producer.ensure_healthy()?;
                    operation(producer.clone(), admission.clone()).await
                })
                .catch_unwind()
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        producer.poison();
                        Err(
                            StreamStoreError::Oplog("durable stream operation panicked".into())
                                .into(),
                        )
                    }
                };
                let status_receipts = std::mem::take(
                    &mut *admission
                        .status_receipts
                        .lock()
                        .expect("status receipt list lock poisoned"),
                );
                for receipt in status_receipts {
                    if let Err(error) = receipt
                        .await
                        .unwrap_or(Err(StreamStoreError::RecoveryRequired))
                    {
                        producer.poison();
                        outcome = Err(error.into());
                    }
                }
                let mut publications = std::mem::take(
                    &mut *admission
                        .publications
                        .lock()
                        .expect("publication receipt list lock poisoned"),
                );
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
                // Lifecycle admission must not be held by an abandoned or backpressured reader.
                drop(admission);
                let _ = reply.send((outcome, publications));
            })))?;
        let (mut outcome, publications) = result
            .await
            .expect("durable stream admitted operation terminated");
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

    async fn submit<T, E, F, Fut>(
        &self,
        admission: Arc<StreamWriteAdmission>,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StreamStoreError> + Send + 'static,
        F: FnOnce(Arc<Self>, StreamWriteContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .try_enter()
            .ok_or(StreamStoreError::RecoveryRequired)?;
        let producer = admission.producer.clone();
        let (reply, result) = oneshot::channel();
        let (status_reply, status_receipt) = oneshot::channel();
        admission
            .status_receipts
            .lock()
            .expect("status receipt list lock poisoned")
            .push(status_receipt);
        let scope = Arc::new(ProducerMutationScope {
            producer: producer.clone(),
            admission,
            commit_tails: std::sync::Mutex::new(Vec::new()),
            session_records_changed: AtomicBool::new(false),
        });
        self.mutations
            .send(MutationJob::Write(Box::pin(async move {
                let outcome = match std::panic::AssertUnwindSafe(async {
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
                        Err(
                            StreamStoreError::Oplog("durable stream mutation panicked".into())
                                .into(),
                        )
                    }
                };
                let _ = reply.send(outcome);
                // The session lock covers the write body, not status publication.
                MutationCompletion {
                    finish: async move {
                        let mut status = Ok(());
                        let tails = std::mem::take(
                            &mut *scope
                                .commit_tails
                                .lock()
                                .expect("commit tail list lock poisoned"),
                        );
                        for tail in tails {
                            if let Err(error) = tail.await {
                                producer.poison();
                                status = Err(StreamStoreError::Oplog(format!(
                                    "durable stream commit callback failed: {error}"
                                )));
                            }
                        }
                        // Slot readers use published worker status, which is folded after the
                        // durability receipt but before the commit callback completes.
                        if scope.session_records_changed.load(Ordering::Acquire) {
                            producer.session_records_changed.notify_waiters();
                        }
                        drop(activity);
                        drop(scope);
                        let _ = status_reply.send(status);
                    }
                    .boxed(),
                }
            })))?;
        result
            .await
            .expect("durable stream producer-owned write terminated")
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
