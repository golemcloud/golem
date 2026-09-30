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

use crate::durable_host::durable_stream::{
    DurableStreamCommit, DurableStreamStore, StreamStoreError,
};
use crate::services::activity::{ActivityGate, ActivityGuard};
use futures::FutureExt;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

type LoadResult = Result<Arc<DurableStreamStore>, StreamStoreError>;
pub(super) type EphemeralArchival = watch::Sender<Option<Result<(), StreamStoreError>>>;

/// Owns initialization and replacement independently of the callers waiting for a producer.
#[derive(Default)]
pub(super) struct DurableStreamProducerSlot {
    state: Mutex<SlotState>,
    state_changed: tokio::sync::Notify,
}

#[derive(Default)]
struct SlotState {
    responses: usize,
    producer: Option<Arc<DurableStreamStore>>,
    deletion_attempt: Option<watch::Receiver<DeletionProducerState>>,
    loading: Option<watch::Receiver<Option<LoadResult>>>,
    failure: Option<StreamStoreError>,
    retired: bool,
    retirement: Option<watch::Receiver<Option<Result<(), StreamStoreError>>>>,
    archival: Option<watch::Receiver<Option<Result<(), StreamStoreError>>>>,
}

#[derive(Clone, Default)]
enum DeletionProducerState {
    #[default]
    Pending,
    Ready {
        producer: Arc<DurableStreamStore>,
        activity: Arc<ActivityGate>,
    },
    Failed(WorkerExecutorError),
    Finished,
}

pub(super) struct DeletionProducerAttempt {
    state: watch::Sender<DeletionProducerState>,
}

pub(super) struct DeletionProducerAccess {
    producer: Arc<DurableStreamStore>,
    _activity: ActivityGuard,
}

impl DeletionProducerAccess {
    pub(super) fn producer(&self) -> &Arc<DurableStreamStore> {
        &self.producer
    }
}

impl DeletionProducerAttempt {
    pub(super) fn publish(&self, producer: Arc<DurableStreamStore>, activity: Arc<ActivityGate>) {
        self.state
            .send_replace(DeletionProducerState::Ready { producer, activity });
    }

    pub(super) fn finish(&self, result: &Result<(), WorkerExecutorError>) {
        self.state.send_replace(match result {
            Ok(()) => DeletionProducerState::Finished,
            Err(error) => DeletionProducerState::Failed(error.clone()),
        });
    }
}

/// Keeps normal ephemeral archival behind the response and all of its stream readers.
/// Explicit owner retirement and executor shutdown do not wait for these leases.
pub struct EphemeralResponseLease {
    slot: Arc<DurableStreamProducerSlot>,
}

impl Drop for EphemeralResponseLease {
    fn drop(&mut self) {
        self.slot.state.lock().unwrap().responses -= 1;
        self.slot.state_changed.notify_waiters();
    }
}

impl DurableStreamProducerSlot {
    pub(super) fn changed(&self) -> &tokio::sync::Notify {
        &self.state_changed
    }

    pub(super) fn retain_response(
        self: &Arc<Self>,
    ) -> Result<Arc<EphemeralResponseLease>, StreamStoreError> {
        let mut state = self.state.lock().unwrap();
        if state.retired {
            return Err(StreamStoreError::RecoveryRequired);
        }
        state.responses += 1;
        Ok(Arc::new(EphemeralResponseLease { slot: self.clone() }))
    }

    /// A missing lease means normal archival completed; the caller must resolve a new owner.
    pub(super) async fn retain_response_or_wait_for_archive(
        self: &Arc<Self>,
    ) -> Result<Option<Arc<EphemeralResponseLease>>, StreamStoreError> {
        let archival = {
            let mut state = self.state.lock().unwrap();
            if !state.retired {
                state.responses += 1;
                return Ok(Some(Arc::new(EphemeralResponseLease {
                    slot: self.clone(),
                })));
            }
            state
                .archival
                .clone()
                .ok_or(StreamStoreError::RecoveryRequired)?
        };
        wait_for_result(archival).await?;
        Ok(None)
    }

    pub(super) async fn wait_for_responses_and_fence(&self) -> Option<EphemeralArchival> {
        loop {
            let changed = self.state_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if state.retired {
                    return None;
                }
                if state.responses == 0 {
                    // Publish the full archive join before another request can see the fence.
                    let (reply, archival) = watch::channel(None);
                    state.archival = Some(archival);
                    state.retired = true;
                    self.state_changed.notify_waiters();
                    if let Some(producer) = &state.producer {
                        producer.poison();
                    }
                    return Some(reply);
                }
            }
            changed.await;
        }
    }

    pub(super) fn is_retired(&self) -> bool {
        self.state.lock().unwrap().retired
    }

    pub(super) fn fence(&self) {
        let mut state = self.state.lock().unwrap();
        state.retired = true;
        if let Some(producer) = &state.producer {
            producer.poison();
        }
        self.state_changed.notify_waiters();
    }

    pub(super) fn begin_deletion_attempt(&self) -> DeletionProducerAttempt {
        let (attempt, state) = watch::channel(DeletionProducerState::Pending);
        self.state.lock().unwrap().deletion_attempt = Some(state);
        self.state_changed.notify_waiters();
        DeletionProducerAttempt { state: attempt }
    }

    pub(super) async fn deletion_producer(
        &self,
    ) -> Result<DeletionProducerAccess, WorkerExecutorError> {
        let mut attempt = loop {
            let changed = self.state_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(attempt) = self.state.lock().unwrap().deletion_attempt.clone() {
                break attempt;
            }
            changed.await;
        };
        loop {
            let state = attempt.borrow().clone();
            match state {
                DeletionProducerState::Pending => {}
                DeletionProducerState::Ready { producer, activity } => {
                    let activity = activity.try_enter().ok_or_else(|| {
                        WorkerExecutorError::invalid_request(
                            "Worker deletion no longer accepts stream controls",
                        )
                    })?;
                    return Ok(DeletionProducerAccess {
                        producer,
                        _activity: activity,
                    });
                }
                DeletionProducerState::Failed(error) => return Err(error),
                DeletionProducerState::Finished => {
                    return Err(WorkerExecutorError::invalid_request(
                        "Worker deletion has finished",
                    ));
                }
            }
            attempt.changed().await.map_err(|_| {
                WorkerExecutorError::runtime("Worker deletion attempt ended without a result")
            })?;
        }
    }

    pub(super) fn retire(
        self: &Arc<Self>,
        commit: DurableStreamCommit,
    ) -> impl Future<Output = Result<(), StreamStoreError>> + Send + 'static {
        self.retire_with_commit(Some(commit))
    }

    /// Fences new work and drains admitted writes without flushing unrelated buffered host calls.
    /// An already-started retirement is joined, including its previously admitted final commit.
    pub(super) fn shutdown(
        self: &Arc<Self>,
    ) -> impl Future<Output = Result<(), StreamStoreError>> + Send + 'static {
        self.retire_with_commit(None)
    }

    fn retire_with_commit(
        self: &Arc<Self>,
        commit: Option<DurableStreamCommit>,
    ) -> impl Future<Output = Result<(), StreamStoreError>> + Send + 'static {
        let retirement = {
            let mut state = self.state.lock().unwrap();
            state.retired = true;
            self.state_changed.notify_waiters();
            if let Some(producer) = &state.producer {
                producer.poison();
            }
            if let Some(retirement) = &state.retirement {
                retirement.clone()
            } else {
                let loading = state.loading.clone();
                let slot = self.clone();
                let (reply, retirement) = watch::channel(None);
                state.retirement = Some(retirement.clone());
                tokio::spawn(async move {
                    let activity = ActivityGate::new();
                    let guard = activity.try_enter().unwrap();
                    let result = std::panic::AssertUnwindSafe(guard.scope(async {
                        if let Some(loading) = loading {
                            // A failed load still has to finish its owned metadata work.
                            let _ = wait_for_result(loading).await;
                        }
                        let producer = slot.state.lock().unwrap().producer.clone();
                        if let Some(producer) = producer {
                            producer.wait_durable_drained().await;
                        }
                        if let Some(commit) = commit {
                            commit(None).await;
                        }
                    }))
                    .catch_unwind()
                    .await
                    .map_err(|_| {
                        StreamStoreError::Oplog(
                            "durable stream producer retirement panicked".to_string(),
                        )
                    });
                    activity.close();
                    activity.wait_drained().await;
                    reply.send_replace(Some(result));
                });
                retirement
            }
        };
        wait_for_result(retirement)
    }

    pub(super) fn try_retire_quiescent(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.responses != 0 {
            return false;
        }
        if state.retired {
            return state
                .retirement
                .as_ref()
                .is_none_or(|retirement| matches!(&*retirement.borrow(), Some(Ok(()))));
        }
        if state.loading.is_some() || state.failure.is_some() {
            return false;
        }
        if let Some(producer) = &state.producer
            && !producer.try_retire_quiescent()
        {
            return false;
        }
        state.retired = true;
        self.state_changed.notify_waiters();
        true
    }

    pub(super) fn has_history(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.producer.is_some() || state.loading.is_some() || state.failure.is_some()
    }

    pub(super) async fn get_or_load<F, Fut>(
        self: &Arc<Self>,
        commit: DurableStreamCommit,
        load: F,
    ) -> LoadResult
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = LoadResult> + Send + 'static,
    {
        let loading = {
            let mut state = self.state.lock().unwrap();
            if state.retired {
                return Err(StreamStoreError::RecoveryRequired);
            }
            if let Some(failure) = &state.failure {
                return Err(failure.clone());
            }
            if let Some(producer) = &state.producer
                && producer.owns_current_activity()
            {
                // A reload drains this producer; a nested lookup must not wait on itself.
                return producer.ensure_healthy().map(|()| producer.clone());
            }
            if let Some(loading) = &state.loading {
                loading.clone()
            } else if let Some(producer) = &state.producer
                && producer.ensure_healthy().is_ok()
            {
                return Ok(producer.clone());
            } else {
                let previous = state.producer.clone();
                let slot = self.clone();
                let (reply, loading) = watch::channel(None);
                state.loading = Some(loading.clone());
                self.state_changed.notify_waiters();
                tokio::spawn(async move {
                    let activity = ActivityGate::new();
                    let guard = activity.try_enter().unwrap();
                    let mut result = std::panic::AssertUnwindSafe(guard.scope(async {
                        if let Some(previous) = previous {
                            previous.poison();
                            previous.wait_durable_drained().await;
                            // Complete the actor's full commit and fold, not its early receipt.
                            commit(None).await;
                        }
                        load().await
                    }))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(StreamStoreError::Oplog(
                            "durable stream producer loading panicked".to_string(),
                        ))
                    });
                    activity.close();
                    activity.wait_drained().await;
                    let mut state = slot.state.lock().unwrap();
                    match &result {
                        Ok(producer) => {
                            if state.retired {
                                producer.poison();
                            }
                            state.producer = Some(producer.clone());
                        }
                        Err(error) if state.producer.is_some() => {
                            state.failure = Some(error.clone());
                        }
                        Err(_) => {}
                    }
                    if state.retired {
                        result = Err(StreamStoreError::RecoveryRequired);
                    }
                    state.loading = None;
                    reply.send_replace(Some(result));
                });
                loading
            }
        };
        wait_for_result(loading).await
    }
}

async fn wait_for_result<T: Clone>(
    mut receiver: watch::Receiver<Option<Result<T, StreamStoreError>>>,
) -> Result<T, StreamStoreError> {
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result;
        }
        receiver.changed().await.map_err(|_| {
            StreamStoreError::Oplog("durable stream producer operation stopped".to_string())
        })?;
    }
}

#[cfg(test)]
mod tests;
