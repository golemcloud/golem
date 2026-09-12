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
    DurableStreamCommit, DurableStreamProducer, DurableStreamProducerError,
};
use crate::services::activity::ActivityGate;
use futures::FutureExt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

type LoadResult = Result<Arc<DurableStreamProducer>, DurableStreamProducerError>;
pub(super) type EphemeralArchival = watch::Sender<Option<Result<(), DurableStreamProducerError>>>;

/// Owns initialization and replacement independently of the callers waiting for a producer.
#[derive(Default)]
pub(super) struct DurableStreamProducerSlot {
    state: Mutex<SlotState>,
    responses_changed: tokio::sync::Notify,
}

#[derive(Default)]
struct SlotState {
    responses: usize,
    producer: Option<Arc<DurableStreamProducer>>,
    loading: Option<watch::Receiver<Option<LoadResult>>>,
    failure: Option<DurableStreamProducerError>,
    retired: bool,
    retirement: Option<watch::Receiver<Option<Result<(), DurableStreamProducerError>>>>,
    archival: Option<watch::Receiver<Option<Result<(), DurableStreamProducerError>>>>,
}

/// Keeps normal ephemeral archival behind the response and all of its stream readers.
/// Explicit owner retirement and executor shutdown do not wait for these leases.
pub(crate) struct EphemeralResponseLease {
    slot: Arc<DurableStreamProducerSlot>,
}

impl Drop for EphemeralResponseLease {
    fn drop(&mut self) {
        self.slot.state.lock().unwrap().responses -= 1;
        self.slot.responses_changed.notify_waiters();
    }
}

impl DurableStreamProducerSlot {
    pub(super) fn retain_response(
        self: &Arc<Self>,
    ) -> Result<Arc<EphemeralResponseLease>, DurableStreamProducerError> {
        let mut state = self.state.lock().unwrap();
        if state.retired {
            return Err(DurableStreamProducerError::RecoveryRequired);
        }
        state.responses += 1;
        Ok(Arc::new(EphemeralResponseLease { slot: self.clone() }))
    }

    /// A missing lease means normal archival completed; the caller must resolve a new owner.
    pub(super) async fn retain_response_or_wait_for_archive(
        self: &Arc<Self>,
    ) -> Result<Option<Arc<EphemeralResponseLease>>, DurableStreamProducerError> {
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
                .ok_or(DurableStreamProducerError::RecoveryRequired)?
        };
        wait_for_result(archival).await?;
        Ok(None)
    }

    pub(super) async fn wait_for_responses_and_fence(&self) -> Option<EphemeralArchival> {
        loop {
            let changed = self.responses_changed.notified();
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
        self.responses_changed.notify_waiters();
    }

    pub(super) fn retire(
        self: &Arc<Self>,
        commit: DurableStreamCommit,
    ) -> impl Future<Output = Result<(), DurableStreamProducerError>> + Send + 'static {
        self.retire_with_commit(Some(commit))
    }

    /// Fences new work and drains admitted writes without flushing unrelated buffered host calls.
    /// An already-started retirement is joined, including its previously admitted final commit.
    pub(super) fn shutdown(
        self: &Arc<Self>,
    ) -> impl Future<Output = Result<(), DurableStreamProducerError>> + Send + 'static {
        self.retire_with_commit(None)
    }

    fn retire_with_commit(
        self: &Arc<Self>,
        commit: Option<DurableStreamCommit>,
    ) -> impl Future<Output = Result<(), DurableStreamProducerError>> + Send + 'static {
        let retirement = {
            let mut state = self.state.lock().unwrap();
            state.retired = true;
            self.responses_changed.notify_waiters();
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
                        DurableStreamProducerError::Oplog(
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
                return Err(DurableStreamProducerError::RecoveryRequired);
            }
            if let Some(failure) = &state.failure {
                return Err(failure.clone());
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
                        Err(DurableStreamProducerError::Oplog(
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
                        result = Err(DurableStreamProducerError::RecoveryRequired);
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
    mut receiver: watch::Receiver<Option<Result<T, DurableStreamProducerError>>>,
) -> Result<T, DurableStreamProducerError> {
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result;
        }
        receiver.changed().await.map_err(|_| {
            DurableStreamProducerError::Oplog(
                "durable stream producer operation stopped".to_string(),
            )
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::tests::{TestOplog, identity};
    use crate::services::activity::spawn_with_activity;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;

    async fn load() -> LoadResult {
        let identity = identity();
        DurableStreamProducer::load(
            Arc::new(TestOplog::default()),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
    }

    fn unused_commit() -> DurableStreamCommit {
        Arc::new(|_| Box::pin(async { panic!("healthy or initial load must not recovery-flush") }))
    }

    #[test]
    async fn ephemeral_archive_waits_for_every_response_reader() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let producer = slot.get_or_load(unused_commit(), load).await.unwrap();
        let response = slot.retain_response().unwrap();
        let reader = response.clone();
        let mut archive = Box::pin(slot.wait_for_responses_and_fence());
        assert!(futures::poll!(archive.as_mut()).is_pending());
        assert!(!slot.try_retire_quiescent());
        producer.ensure_healthy().unwrap();
        drop(response);
        assert!(futures::poll!(archive.as_mut()).is_pending());
        let second_response = slot
            .retain_response_or_wait_for_archive()
            .await
            .unwrap()
            .unwrap();
        drop(reader);
        assert!(futures::poll!(archive.as_mut()).is_pending());
        drop(second_response);
        let archival = archive.await.unwrap();
        assert!(producer.ensure_healthy().is_err());
        assert!(slot.retain_response().is_err());
        let mut response = Box::pin(slot.retain_response_or_wait_for_archive());
        assert!(futures::poll!(response.as_mut()).is_pending());
        archival.send_replace(Some(Ok(())));
        assert!(response.await.unwrap().is_none());
        // The old owner remains fenced even after a replacement may be resolved.
        assert!(slot.retain_response().is_err());
        assert!(producer.ensure_healthy().is_err());
    }

    #[test]
    async fn failed_or_aborted_ephemeral_archive_rejects_response_admission() {
        for abort in [false, true] {
            let slot = Arc::new(DurableStreamProducerSlot::default());
            let archival = slot.wait_for_responses_and_fence().await.unwrap();
            let mut response = Box::pin(slot.retain_response_or_wait_for_archive());
            assert!(futures::poll!(response.as_mut()).is_pending());
            if !abort {
                archival.send_replace(Some(Err(DurableStreamProducerError::Oplog(
                    "archive failed".to_string(),
                ))));
            }
            drop(archival);
            assert!(response.await.is_err());
        }
    }

    #[test]
    async fn explicit_retirement_does_not_wait_for_ephemeral_responses() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let producer = slot.get_or_load(unused_commit(), load).await.unwrap();
        let _response = slot.retain_response().unwrap();
        let mut archive = Box::pin(slot.wait_for_responses_and_fence());
        assert!(futures::poll!(archive.as_mut()).is_pending());
        slot.fence();
        assert!(archive.await.is_none());
        assert!(slot.retain_response_or_wait_for_archive().await.is_err());
        assert!(producer.ensure_healthy().is_err());
        assert!(slot.retain_response().is_err());
        slot.shutdown().await.unwrap();
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn executor_shutdown_drains_deferred_ephemeral_archive() {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let loops = crate::services::active_agents::InvocationLoops::new(shutdown.clone());
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let producer = slot.get_or_load(unused_commit(), load).await.unwrap();
        let _response = slot.retain_response().unwrap();
        let archive_slot = slot.clone();
        let shutdown_slot = slot.clone();
        loops.spawn(
            async move {
                archive_slot.wait_for_responses_and_fence().await;
            },
            move || {
                let drain = shutdown_slot.shutdown();
                Box::pin(async move { drain.await.unwrap() })
            },
        );
        shutdown.cancel();
        loops.wait_for_exit().await;
        assert!(producer.ensure_healthy().is_err());
        assert!(slot.retain_response().is_err());
    }

    #[test]
    async fn shutdown_fences_empty_and_idle_slots_without_flushing_buffered_entries() {
        use crate::services::oplog::{CommitLevel, Oplog};
        use golem_common::model::oplog::OplogEntry;

        for initialized in [false, true] {
            let slot = Arc::new(DurableStreamProducerSlot::default());
            let oplog = Arc::new(TestOplog::default());
            if initialized {
                let identity = identity();
                let source = oplog.clone();
                slot.get_or_load(unused_commit(), move || async move {
                    DurableStreamProducer::load(
                        source,
                        identity.environment_id,
                        identity.agent_id,
                        identity.fingerprint,
                        None,
                    )
                    .await
                })
                .await
                .unwrap();
            }
            oplog
                .add(OplogEntry::NoOp {
                    timestamp: golem_common::model::Timestamp::now_utc(),
                    entity_parent_start_index: None,
                })
                .await;
            let shutdown = slot.shutdown();
            assert!(slot.is_retired());
            shutdown.await.unwrap();
            slot.retire(unused_commit()).await.unwrap();
            assert_eq!(oplog.commit(CommitLevel::Always).await.len(), 1);
        }
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn shutdown_waits_for_admitted_commit_tail_after_cancelled_waiter() {
        use crate::durable_host::durable_stream::tests::registration;
        use crate::services::oplog::{CommitLevel, Oplog};
        use golem_common::base_model::durable_stream::{
            StreamRegistrationCoordinateV1, StreamRootKindV1, StreamSourceKindV1,
        };

        let slot = Arc::new(DurableStreamProducerSlot::default());
        let identity = identity();
        let request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![],
            },
            StreamSourceKindV1::InvocationOutput,
        );
        let oplog = Arc::new(TestOplog::default());
        let reached = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let commit: DurableStreamCommit = {
            let oplog = oplog.clone();
            let reached = reached.clone();
            let release = release.clone();
            Arc::new(move |receipt| {
                let oplog = oplog.clone();
                let reached = reached.clone();
                let release = release.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    receipt.unwrap().send(()).unwrap();
                    reached.notify_one();
                    release.acquire().await.unwrap().forget();
                })
            })
        };
        let producer = slot
            .get_or_load(unused_commit(), move || async move {
                DurableStreamProducer::load_with_commit(
                    oplog,
                    identity.environment_id,
                    identity.agent_id,
                    identity.fingerprint,
                    None,
                    commit,
                )
                .await
            })
            .await
            .unwrap();
        let mut append = Box::pin(producer.register(request));
        assert!(futures::poll!(append.as_mut()).is_pending());
        reached.notified().await;
        drop(append);
        drop(slot.shutdown());
        assert_eq!(
            producer.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        );
        let mut shutdown = Box::pin(slot.shutdown());
        assert!(futures::poll!(shutdown.as_mut()).is_pending());
        release.add_permits(1);
        shutdown.await.unwrap();
        assert!(slot.try_retire_quiescent());
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn forced_retirement_wins_initial_and_recovery_publication_after_metadata_drains() {
        for recovering in [false, true] {
            let slot = Arc::new(DurableStreamProducerSlot::default());
            if recovering {
                slot.get_or_load(unused_commit(), load)
                    .await
                    .unwrap()
                    .poison();
            }
            let producer = load().await.unwrap();
            let unpublished = producer.clone();
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, released) = tokio::sync::oneshot::channel();
            let mut loading = Box::pin(slot.get_or_load(
                Arc::new(|_| Box::pin(async {})),
                move || async move {
                    spawn_with_activity(async move {
                        released.await.unwrap();
                    });
                    started.send(()).unwrap();
                    Ok(producer)
                },
            ));
            assert!(futures::poll!(loading.as_mut()).is_pending());
            ready.await.unwrap();
            let flushed = Arc::new(AtomicBool::new(false));
            let flushed_callback = flushed.clone();
            let first = slot.retire(Arc::new(move |_| {
                let flushed = flushed_callback.clone();
                Box::pin(async move {
                    flushed.store(true, Ordering::Release);
                })
            }));
            drop(first);
            assert!(!slot.try_retire_quiescent());
            assert!(matches!(
                slot.get_or_load(unused_commit(), load).await,
                Err(DurableStreamProducerError::RecoveryRequired)
            ));
            let mut retirement = Box::pin(slot.retire(unused_commit()));
            assert!(futures::poll!(retirement.as_mut()).is_pending());
            assert!(!flushed.load(Ordering::Acquire));
            release.send(()).unwrap();
            assert!(matches!(
                loading.await,
                Err(DurableStreamProducerError::RecoveryRequired)
            ));
            retirement.await.unwrap();
            assert!(flushed.load(Ordering::Acquire));
            assert_eq!(
                unpublished.ensure_healthy(),
                Err(DurableStreamProducerError::RecoveryRequired)
            );
            assert!(slot.try_retire_quiescent());
        }
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn cancelled_retirement_waiter_does_not_cancel_the_final_flush() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let producer = slot.get_or_load(unused_commit(), load).await.unwrap();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let first = slot.retire({
            let release = release.clone();
            let started = started.clone();
            Arc::new(move |_| {
                let release = release.clone();
                let started = started.clone();
                Box::pin(async move {
                    started.notify_one();
                    release.acquire().await.unwrap().forget();
                })
            })
        });
        assert_eq!(
            producer.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        );
        drop(first);
        started.notified().await;
        assert!(!slot.try_retire_quiescent());
        let mut second = Box::pin(slot.retire(unused_commit()));
        assert!(futures::poll!(second.as_mut()).is_pending());
        release.add_permits(1);
        second.await.unwrap();
        assert!(slot.try_retire_quiescent());
    }

    #[test]
    async fn quiescent_retirement_fences_both_empty_and_loaded_slots() {
        for initialized in [false, true] {
            let slot = Arc::new(DurableStreamProducerSlot::default());
            let previous = if initialized {
                Some(slot.get_or_load(unused_commit(), load).await.unwrap())
            } else {
                None
            };
            assert!(slot.try_retire_quiescent());
            assert!(slot.try_retire_quiescent());
            let result = slot
                .get_or_load(unused_commit(), || async {
                    panic!("retired slot must not start loading")
                })
                .await;
            assert!(matches!(
                result,
                Err(DurableStreamProducerError::RecoveryRequired)
            ));
            if let Some(previous) = previous {
                assert_eq!(
                    previous.ensure_healthy(),
                    Err(DurableStreamProducerError::RecoveryRequired)
                );
            }
        }
    }

    #[test]
    async fn failed_initial_load_can_be_retried() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let first = slot
            .get_or_load(unused_commit(), || async {
                Err(DurableStreamProducerError::Oplog(
                    "transient metadata lookup failure".to_string(),
                ))
            })
            .await;
        assert!(first.is_err());

        let producer = slot
            .get_or_load(unused_commit(), load)
            .await
            .expect("an initial load failure must not permanently fence the slot");
        assert_eq!(producer.ensure_healthy(), Ok(()));
    }

    #[test]
    async fn failed_recovery_load_keeps_the_owner_fenced() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        slot.get_or_load(unused_commit(), load)
            .await
            .unwrap()
            .poison();

        let first = slot
            .get_or_load(Arc::new(|_| Box::pin(async {})), || async {
                Err(DurableStreamProducerError::Oplog(
                    "transient metadata lookup failure".to_string(),
                ))
            })
            .await;
        assert!(first.is_err());

        let retry = slot
            .get_or_load(unused_commit(), || async {
                panic!("a failed recovery must not publish another producer for this owner")
            })
            .await;
        assert_eq!(retry.err(), first.err());
        assert!(!slot.try_retire_quiescent());
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn cancelled_initial_waiter_does_not_cancel_load_or_detached_metadata() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let producer = load().await.unwrap();
        let expected = producer.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let mut first = Box::pin(slot.get_or_load(unused_commit(), move || async move {
            spawn_with_activity(async move {
                released.await.unwrap();
            });
            started.send(()).unwrap();
            Ok(producer)
        }));
        assert!(futures::poll!(first.as_mut()).is_pending());
        ready.await.unwrap();
        assert!(!slot.try_retire_quiescent());
        drop(first);
        let mut joined = Box::pin(slot.get_or_load(unused_commit(), || async {
            panic!("second waiter started another load")
        }));
        assert!(futures::poll!(joined.as_mut()).is_pending());
        assert!(slot.has_history());
        release.send(()).unwrap();
        assert!(Arc::ptr_eq(&joined.await.unwrap(), &expected));
        let cached = slot
            .get_or_load(unused_commit(), || async { panic!("healthy reload") })
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&cached, &expected));
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn recovery_is_single_flight_and_waits_for_the_full_flush_after_cancellation() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let old = slot.get_or_load(unused_commit(), load).await.unwrap();
        old.poison();
        let flushed = Arc::new(AtomicBool::new(false));
        let (started, ready) = tokio::sync::oneshot::channel();
        let started = Arc::new(Mutex::new(Some(started)));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let commit: DurableStreamCommit = {
            let release = release.clone();
            let flushed = flushed.clone();
            Arc::new(move |receipt| {
                assert!(receipt.is_none());
                let release = release.clone();
                let flushed = flushed.clone();
                let started = started.lock().unwrap().take().unwrap();
                Box::pin(async move {
                    started.send(()).unwrap();
                    release.acquire().await.unwrap().forget();
                    flushed.store(true, Ordering::Release);
                })
            })
        };
        let mut first = Box::pin(slot.get_or_load(commit, move || async move {
            assert!(
                flushed.load(Ordering::Acquire),
                "reload before full commit/fold"
            );
            load().await
        }));
        assert!(futures::poll!(first.as_mut()).is_pending());
        ready.await.unwrap();
        assert!(!slot.try_retire_quiescent());
        drop(first);
        let mut second =
            Box::pin(slot.get_or_load(unused_commit(), || async { panic!("duplicate recovery") }));
        assert!(futures::poll!(second.as_mut()).is_pending());
        assert_eq!(
            old.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        );
        release.add_permits(1);
        let replacement = second.await.unwrap();
        assert!(!Arc::ptr_eq(&replacement, &old));
        assert_eq!(replacement.ensure_healthy(), Ok(()));
        assert_eq!(
            old.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        );
    }

    #[test]
    async fn corrupt_recovery_stays_fenced_without_automatic_retries() {
        let slot = Arc::new(DurableStreamProducerSlot::default());
        let old = slot.get_or_load(unused_commit(), load).await.unwrap();
        old.poison();
        let expected = DurableStreamProducerError::CorruptHistory("invalid batch".to_string());
        assert!(!slot.try_retire_quiescent());
        let error = expected.clone();
        let result = slot
            .get_or_load(Arc::new(|_| Box::pin(async {})), move || async {
                Err(error)
            })
            .await;
        assert!(matches!(result, Err(error) if error == expected));
        assert!(!slot.try_retire_quiescent());
        let result = slot
            .get_or_load(unused_commit(), || async {
                panic!("corrupt recovery was retried")
            })
            .await;
        assert!(matches!(result, Err(error) if error == expected));
        assert_eq!(
            old.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        );
    }
}
