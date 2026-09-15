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

use super::probe::ConsumerJournalInspection;
use super::registration::registration_record;
use super::{
    AgentError, AttachedStreamSegmentSource, CommittedProducerStreamEvent,
    CommittedProducerStreamEventPayload, ConsumerAttachmentStatus, ConsumerJournalSummary,
    DurableCatchUpReader, DurableLiveStreamBus, DurableLiveStreamBusError, DurableStreamCommit,
    DurableStreamStore, ExternalAppendOutcome, ExternalProducer, IndexedConsumerJournal,
    ProducerMetadataKey, ProducerOutputRegistration, ProducerOutputSource,
    ProducerRegistrationRequest, ProducerStreamIndex, ResultStreamRegistration,
    StreamAttachmentConsumerProbe, StreamAttachmentControl, StreamAttachmentState,
    StreamSegmentSource, StreamStoreError,
};
use crate::services::oplog::{
    CommitLevel, DurableStreamOplogRecord, Oplog, OplogAddReceipt, OplogReadSource,
    OrderedOplogStart, PendingUpload, RawDurableStreamSessionStatus, checked_range_end,
    exact_from_source, fail_stop,
};
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::base_model::component::{ComponentId, ComponentRevision};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, ExternalProducerId,
    InputStreamHighWater, MAX_DURABLE_STREAM_ITEM_SIZE, MAX_DURABLE_STREAMS_PER_SESSION,
    MAX_LIVE_JOIN_BUFFER_SIZE, MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
    MAX_STREAM_VALUE_TRAVERSAL_DEPTH, PersistedStreamInvocationDescriptor,
    STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS, STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
    SessionStreamRole, StartAttemptDescriptor, StreamAttachmentFinalizationReason,
    StreamAttachmentKey, StreamCancelReason, StreamCancelRole, StreamCascadeDependentResult,
    StreamConsumerDeletingRecord, StreamConsumerItemValueRecord, StreamEndResult, StreamId,
    StreamInvocationId, StreamItemsPayload, StreamItemsRecord, StreamOffset,
    StreamRegistrationCoordinate, StreamRootKind, StreamSessionKey, StreamSessionMappingRecord,
    StreamSessionMappingUpdateRecord, StreamSessionPreparedRecord, StreamSessionRecord,
    StreamSourceKind, StreamTerminalAuthor, StreamTopologyActivatedRecord,
    StreamTopologyPreparedRecord, StreamValuePathStep,
};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
use golem_common::model::AgentInvocationPayload;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::payload::OplogPayload;
use golem_common::model::oplog::{OplogEntry, PayloadId, RawOplogPayload};
use golem_schema::schema::SchemaFingerprintV1;
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::sync::{Barrier, Notify, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
struct TestOplogState {
    entries: BTreeMap<OplogIndex, OplogEntry>,
    committed: OplogIndex,
    commit_count: u64,
}

#[derive(Default)]
pub(crate) struct TestOplog {
    state: Mutex<TestOplogState>,
    read_ranges: Mutex<Vec<(OplogIndex, u64)>>,
    point_reads: AtomicU64,
}

impl Debug for TestOplog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TestOplog").finish()
    }
}

impl TestOplog {
    pub(crate) fn take_read_ranges(&self) -> Vec<(OplogIndex, u64)> {
        std::mem::take(&mut *self.read_ranges.lock().unwrap())
    }

    fn committed_length(&self) -> u64 {
        self.state.lock().unwrap().committed.as_u64()
    }

    fn commit_count(&self) -> u64 {
        self.state.lock().unwrap().commit_count
    }

    fn entries(&self) -> Vec<OplogEntry> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl Oplog for TestOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        let mut state = self.state.lock().unwrap();
        let index = state
            .entries
            .last_key_value()
            .map_or(OplogIndex::INITIAL, |(index, _)| index.next());
        state.entries.insert(index, entry);
        index
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let mut state = self.state.lock().unwrap();
        let index = state
            .entries
            .last_key_value()
            .map_or(OplogIndex::INITIAL, |(index, _)| index.next());
        state.entries.insert(index, entry);
        Box::pin(async move { index })
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let mut state = self.state.lock().unwrap();
        let before = state.entries.len();
        state.entries.retain(|index, _| *index > last_dropped_id);
        (before - state.entries.len()) as u64
    }

    async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut state = self.state.lock().unwrap();
        let committed = state
            .entries
            .iter()
            .filter(|(index, _)| **index > state.committed)
            .map(|(index, entry)| (*index, entry.clone()))
            .collect();
        state.committed = state
            .entries
            .last_key_value()
            .map_or(state.committed, |(index, _)| *index);
        state.commit_count += 1;
        committed
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.state
            .lock()
            .unwrap()
            .entries
            .last_key_value()
            .map_or(OplogIndex::NONE, |(index, _)| *index)
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &StreamInvocationId,
    ) -> RawDurableStreamSessionStatus {
        let state = self.state.lock().unwrap();
        let watermark = state
            .entries
            .last_key_value()
            .map_or(OplogIndex::NONE, |(i, _)| *i);
        let mut status = golem_common::model::DurableStreamSessionStatus {
            session_key: Some(session_key.clone()),
            ..Default::default()
        };
        for (index, entry) in &state.entries {
            if let OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            } = entry
            {
                status.apply_pending_invocation(*index, idempotency_key);
            }
            if let OplogEntry::StreamSession { record, .. } = entry {
                let record = match record {
                    OplogPayload::Inline(record) => record.as_ref(),
                    OplogPayload::SerializedInline {
                        cached: Some(record),
                        ..
                    }
                    | OplogPayload::External {
                        cached: Some(record),
                        ..
                    } => record.as_ref(),
                    _ => continue,
                };
                status.apply_record(*index, record);
            }
        }
        RawDurableStreamSessionStatus {
            watermark,
            status: Ok(Some(status)),
        }
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.state
            .lock()
            .unwrap()
            .entries
            .iter()
            .rev()
            .find_map(|(index, entry)| (!entry.is_hint()).then_some(*index))
    }

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        true
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.point_reads.fetch_add(1, Ordering::Relaxed);
        self.state
            .lock()
            .unwrap()
            .entries
            .get(&oplog_index)
            .cloned()
            .expect("missing test oplog entry")
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_ranges.lock().unwrap().push((oplog_index, n));
        let state = self.state.lock().unwrap();
        let end = fail_stop(checked_range_end(oplog_index, n));
        let entries = end.map_or_else(BTreeMap::new, |end| {
            state
                .entries
                .range(oplog_index..=end)
                .map(|(index, entry)| (*index, entry.clone()))
                .collect()
        });
        fail_stop(exact_from_source(
            OplogReadSource::Other("durable stream test oplog"),
            oplog_index,
            n,
            entries,
        ))
    }

    async fn length(&self) -> u64 {
        self.state.lock().unwrap().entries.len() as u64
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        Ok(RawOplogPayload::SerializedInline(data))
    }

    async fn download_raw_payload(
        &self,
        _payload_id: PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        Err("test oplog has no external payloads".to_string())
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
    ) -> Result<OrderedOplogStart, String> {
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        let index = self.add(entry.clone()).await;
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: crate::services::oplog::IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, String> {
        let mut state = self.state.lock().unwrap();
        let index = state
            .entries
            .last_key_value()
            .map_or(OplogIndex::INITIAL, |(index, _)| index.next());
        let (serialized_request, build_start) = build_request(index)?;
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        state.entries.insert(index, entry.clone());
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        let first = self.add(start).await;
        let second = self.add(make_second(first)).await;
        (first, second)
    }
}

#[test]
async fn test_oplog_read_exact_includes_uncommitted_entries() {
    let oplog = TestOplog::default();
    let entry = OplogEntry::interrupted();
    let index = oplog.add(entry.clone()).await;

    let entries = oplog.read_exact(index, 1).await;

    assert_eq!(entries.get(&index), Some(&entry));
}

#[test]
async fn test_oplog_read_exact_rejects_incomplete_range() {
    let oplog = TestOplog::default();
    let index = oplog.add(OplogEntry::interrupted()).await;

    let result = std::panic::AssertUnwindSafe(oplog.read_exact(index, 2))
        .catch_unwind()
        .await;

    assert!(
        result.is_err(),
        "read_exact accepted a range whose second entry is missing"
    );
}

#[test]
async fn test_oplog_read_exact_accepts_single_entry_at_max_index() {
    let oplog = TestOplog::default();
    let index = OplogIndex::from_u64(u64::MAX);
    let entry = OplogEntry::interrupted();
    oplog
        .state
        .lock()
        .unwrap()
        .entries
        .insert(index, entry.clone());

    let entries = oplog.read_exact(index, 1).await;

    assert_eq!(entries.get(&index), Some(&entry));
}

#[test]
#[timeout("30s")]
async fn mutation_queue_serializes_abandoned_requests_on_one_task() {
    let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let (entered, ready) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let first_finished = Arc::new(AtomicBool::new(false));
    let first = tokio::spawn({
        let live = live.clone();
        let first_finished = first_finished.clone();
        async move {
            live.run_owned(None, 0, move |_, context| async move {
                let task = tokio::task::id();
                entered.send(task).unwrap();
                released.await.unwrap();
                // Nested mutations execute inline rather than enqueueing behind themselves.
                context
                    .run_nested(|_, _| async { Ok::<(), StreamStoreError>(()) })
                    .await?;
                first_finished.store(true, Ordering::Release);
                Ok::<_, StreamStoreError>(task)
            })
            .await
        }
    });
    let task = ready.await.unwrap();
    let (completed, completion) = oneshot::channel();
    let mut second = Box::pin(live.run_owned(None, 0, move |_, _| async move {
        assert!(first_finished.load(Ordering::Acquire));
        assert_eq!(tokio::task::id(), task);
        completed.send(()).unwrap();
        Ok::<(), StreamStoreError>(())
    }));
    assert!(futures::poll!(second.as_mut()).is_pending());
    drop(second);
    release.send(()).unwrap();
    assert_eq!(first.await.unwrap().unwrap(), task);
    completion.await.unwrap();
    live.wait_durable_drained().await;
    live.ensure_healthy().unwrap();
}

#[test]
#[timeout("30s")]
async fn mutation_queue_drains_active_work_and_rejects_queued_work_after_retirement() {
    let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let (entered, ready) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let first = tokio::spawn({
        let live = live.clone();
        async move {
            live.run_owned(None, 0, move |_, context| async move {
                context.begin_durable_effect();
                entered.send(()).unwrap();
                released.await.unwrap();
                context.finish_durable_effect();
                Ok::<(), StreamStoreError>(())
            })
            .await
        }
    });
    ready.await.unwrap();
    let executed = Arc::new(AtomicBool::new(false));
    let queued_executed = executed.clone();
    let mut queued = Box::pin(live.run_owned(None, 0, move |_, _| async move {
        queued_executed.store(true, Ordering::Release);
        Ok::<(), StreamStoreError>(())
    }));
    assert!(futures::poll!(queued.as_mut()).is_pending());
    live.poison();
    let mut drained = Box::pin(live.wait_durable_drained());
    assert!(futures::poll!(drained.as_mut()).is_pending());
    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    assert_eq!(queued.await, Err(StreamStoreError::RecoveryRequired));
    drained.await;
    assert!(!executed.load(Ordering::Acquire));
    assert_eq!(
        live.run_owned(None, 0, |_, _| async { Ok::<(), StreamStoreError>(()) })
            .await,
        Err(StreamStoreError::RecoveryRequired)
    );
}

#[test]
#[timeout("30s")]
async fn mutation_queue_completion_can_route_back_to_the_same_producer() {
    let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let (entered, ready) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (completed, completion) = oneshot::channel();
    let mut caller =
        Box::pin(
            live.run_admitted(None, 0, true, move |owner, admission| async move {
                admission
                    .submit(|_, _| async { Ok::<(), StreamStoreError>(()) })
                    .await?;
                entered.send(()).unwrap();
                released.await.unwrap();
                owner
                    .run_owned(None, 0, |_, _| async { Ok::<(), StreamStoreError>(()) })
                    .await?;
                completed.send(()).unwrap();
                Ok::<(), StreamStoreError>(())
            }),
        );
    assert!(futures::poll!(caller.as_mut()).is_pending());
    ready.await.unwrap();
    drop(caller);
    // A stalled completion does not hold the serial mutation lane.
    live.run_lifecycle(None, 0, |_, _| async { Ok::<(), StreamStoreError>(()) })
        .await
        .unwrap();
    release.send(()).unwrap();
    completion.await.unwrap();
    live.wait_durable_drained().await;
    live.ensure_healthy().unwrap();
}

#[test]
#[timeout("30s")]
async fn write_context_rejects_a_different_store_and_use_after_completion() {
    let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let other = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let escaped = live
        .run_owned(None, 0, move |owner, context| async move {
            context.assert_owner(&owner);
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    context.assert_owner(&other);
                }))
                .is_err()
            );
            assert_eq!(other.ensure_healthy(), Ok(()));
            Ok::<_, StreamStoreError>(context)
        })
        .await
        .unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            escaped.begin_durable_effect();
        }))
        .is_err()
    );
    assert_eq!(live.ensure_healthy(), Ok(()));
}

#[test]
#[timeout("30s")]
async fn saturated_admitted_operations_finish_after_callers_disconnect() {
    for lifecycle in [false, true] {
        let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
        let entered = Arc::new(tokio::sync::Barrier::new(17));
        let release = Arc::new(tokio::sync::Barrier::new(17));
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (completed, mut completion) = tokio::sync::mpsc::unbounded_channel();
        let mut callers = Vec::new();
        for _ in 0..16 {
            let live = live.clone();
            let entered = entered.clone();
            let release = release.clone();
            let writes = writes.clone();
            let completed = completed.clone();
            callers.push(tokio::spawn(async move {
                live.run_admitted(None, 7, lifecycle, move |_, admission| async move {
                    let first = writes.clone();
                    admission
                        .submit(move |_, _| async move {
                            first.fetch_add(1, Ordering::SeqCst);
                            Ok::<(), StreamStoreError>(())
                        })
                        .await?;
                    entered.wait().await;
                    release.wait().await;
                    admission
                        .submit(move |_, _| async move {
                            writes.fetch_add(1, Ordering::SeqCst);
                            Ok::<(), StreamStoreError>(())
                        })
                        .await?;
                    completed.send(()).unwrap();
                    Ok::<(), StreamStoreError>(())
                })
                .await
            }));
        }
        entered.wait().await;
        assert_eq!(writes.load(Ordering::SeqCst), 16);
        let lane = if lifecycle {
            &live.lifecycle_operations
        } else {
            &live.owned_operations
        };
        assert_eq!(lane.available_permits(), 0);
        for caller in callers {
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
        }
        release.wait().await;
        for _ in 0..16 {
            completion.recv().await.unwrap();
        }
        let all_permits = lane.clone().acquire_many_owned(16).await.unwrap();
        assert_eq!(writes.load(Ordering::SeqCst), 32);
        assert_eq!(live.ensure_healthy(), Ok(()));
        drop(all_permits);
    }
}

#[test]
#[timeout("30s")]
async fn admitted_peer_waits_do_not_block_each_others_local_writer() {
    let left = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let right = producer(Arc::new(TestOplog::default()), &identity(), None).await;
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for (local, remote) in [(left.clone(), right.clone()), (right, left)] {
        let entered = entered.clone();
        tasks.push(tokio::spawn(async move {
            local
                .run_admitted(None, 0, false, move |owner, admission| async move {
                    admission
                        .submit(|_, _| async { Ok::<(), StreamStoreError>(()) })
                        .await?;
                    entered.wait().await;
                    let remote_value =
                        owner
                            .remote_until_retired(remote.run_lifecycle(None, 0, |_, _| async {
                                Ok::<_, StreamStoreError>(37)
                            }))
                            .await?;
                    admission
                        .submit(
                            move |_, _| async move { Ok::<_, StreamStoreError>(remote_value + 5) },
                        )
                        .await
                })
                .await
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap(), 42);
    }
}

#[test]
#[timeout("30s")]
async fn publication_waits_until_admitted_session_lock_is_released() {
    for lifecycle in [false, true] {
        let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let (published, publication) = oneshot::channel();
        let (written, write_done) = oneshot::channel();
        let operation = tokio::spawn({
            let live = live.clone();
            let lock = lock.clone();
            async move {
                live.run_admitted(None, 11, lifecycle, move |_, admission| async move {
                    let _guard = lock.lock().await;
                    admission
                        .submit(move |_, context| async move {
                            context.defer_publication(Box::pin(async move {
                                Ok(publication.await.unwrap())
                            }));
                            Ok::<(), StreamStoreError>(())
                        })
                        .await?;
                    written.send(()).unwrap();
                    Ok::<(), StreamStoreError>(())
                })
                .await
            }
        });
        write_done.await.unwrap();
        let _guard = lock.lock().await;
        if lifecycle {
            let permits = live
                .lifecycle_operations
                .clone()
                .acquire_many_owned(16)
                .await
                .unwrap();
            assert!(!operation.is_finished());
            drop(permits);
        } else {
            assert_eq!(live.owned_operations.available_permits(), 15);
        }
        published.send(Ok(())).unwrap();
        operation.await.unwrap().unwrap();
    }
}

#[test]
#[timeout("30s")]
async fn session_finish_holds_its_lock_and_reserves_terminal_batch_bytes() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let reached = reached.clone();
        let release = release.clone();
        move |published| {
            let oplog = oplog.clone();
            let reached = reached.clone();
            let release = release.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                reached.notify_one();
                release.notified().await;
                if let Some(published) = published {
                    published.send(()).unwrap();
                }
            })
        }
    });
    let live = DurableStreamStore::load_with_commit(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let session = crate::durable_host::durable_session::StreamSession::new(
        live.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [],
    );
    let lock = live.session_lock(&identity.invocation);
    let guard = lock.lock().await;
    let mut finish = Box::pin(session.fail("x".repeat(300 * 1024)));
    assert!(futures::poll!(finish.as_mut()).is_pending());
    // A 300 KiB error copied across the maximum terminal batch exhausts the byte lane.
    assert_eq!(live.lifecycle_operation_bytes.available_permits(), 0);
    drop(guard);
    let (result, ()) = tokio::join!(finish, async {
        reached.notified().await;
        assert!(
            lock.try_lock().is_err(),
            "finish released the session lock before its commit"
        );
        release.notify_one();
    });
    result.unwrap();
    assert!(lock.try_lock().is_ok());
    assert_eq!(
        live.lifecycle_operation_bytes.available_permits(),
        256 * 1024 * 1024
    );
    assert!(
        live.index_for([ProducerMetadataKey::Session(identity.invocation.clone())])
            .await
            .unwrap()
            .finished_sessions
            .contains(&identity.invocation)
    );
}

#[test]
async fn nested_mutation_preserves_unfinished_parent_effects() {
    for parent_pending in [false, true] {
        for child_succeeds in [false, true] {
            for child_finishes in [false, true] {
                let identity = identity();
                let producer = DurableStreamStore::load(
                    Arc::new(TestOplog::default()),
                    identity.environment_id,
                    identity.agent_id,
                    identity.fingerprint,
                    None,
                )
                .await
                .unwrap();
                let outcome: Result<(), StreamStoreError> = producer
                    .run_owned(None, 0, move |_, parent| async move {
                        if parent_pending {
                            parent.begin_durable_effect();
                        }
                        let child_result: Result<(), StreamStoreError> = parent
                            .run_nested(move |_, child| async move {
                                child.begin_durable_effect();
                                if child_finishes {
                                    child.finish_durable_effect();
                                }
                                if child_succeeds {
                                    Ok(())
                                } else {
                                    Err(StreamStoreError::ItemTooLarge)
                                }
                            })
                            .await;
                        assert_eq!(child_result.is_ok(), child_succeeds);
                        Err(StreamStoreError::ItemTooLarge)
                    })
                    .await;
                assert!(outcome.is_err());
                let requires_recovery = parent_pending || (!child_succeeds && !child_finishes);
                assert_eq!(
                    producer.ensure_healthy().is_err(),
                    requires_recovery,
                    "parent_pending={parent_pending}, child_succeeds={child_succeeds}, child_finishes={child_finishes}"
                );
            }
        }
    }
}

#[test]
#[test_r::timeout("10s")]
async fn nested_sibling_success_cannot_hide_failed_or_cancelled_effects() {
    for cancel_child in [false, true] {
        let identity = identity();
        let producer = DurableStreamStore::load(
            Arc::new(TestOplog::default()),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        producer
            .run_owned(None, 0, move |owner, parent| async move {
                let (release, released) = oneshot::channel();
                let mut child = Box::pin(parent.run_nested(move |_, child| async move {
                    child.begin_durable_effect();
                    released.await.unwrap();
                    Err::<(), _>(StreamStoreError::ItemTooLarge)
                }));
                assert!(futures::poll!(child.as_mut()).is_pending());
                parent
                    .run_nested(|_, sibling| async move {
                        sibling.begin_durable_effect();
                        sibling.finish_durable_effect();
                        Ok::<(), StreamStoreError>(())
                    })
                    .await?;
                assert_eq!(owner.ensure_healthy(), Ok(()));
                if cancel_child {
                    drop(child);
                } else {
                    release.send(()).unwrap();
                    assert!(child.await.is_err());
                }
                Ok::<(), StreamStoreError>(())
            })
            .await
            .unwrap();
        assert_eq!(
            producer.ensure_healthy(),
            Err(StreamStoreError::RecoveryRequired)
        );
    }
}

#[test]
async fn quiescent_retirement_rejects_storage_activity_without_poisoning_the_producer() {
    let identity = identity();
    let producer = DurableStreamStore::load(
        Arc::new(TestOplog::default()),
        identity.environment_id,
        identity.agent_id,
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let activity = producer.durable_activity.try_enter().unwrap();
    assert!(!producer.try_retire_quiescent());
    assert_eq!(producer.ensure_healthy(), Ok(()));
    assert!(producer.durable_activity.try_enter().is_some());
    drop(activity);
    assert!(producer.try_retire_quiescent());
    assert_eq!(
        producer.ensure_healthy(),
        Err(StreamStoreError::RecoveryRequired)
    );
    assert!(producer.durable_activity.try_enter().is_none());
}

#[test]
#[test_r::timeout("10s")]
async fn metadata_lookup_tracks_detached_storage_after_caller_cancellation() {
    for cancel_caller in [false, true] {
        let identity = identity();
        let producer = DurableStreamStore::load(
            Arc::new(TestOplog::default()),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let (release, released) = oneshot::channel();
        let (started, ready) = oneshot::channel();
        let mut lookup = Box::pin(producer.with_metadata_activity(async {
            crate::services::activity::spawn_with_activity(async move {
                started.send(()).unwrap();
                released.await.unwrap();
            })
            .await
            .unwrap();
            37
        }));
        assert!(futures::poll!(lookup.as_mut()).is_pending());
        ready.await.unwrap();
        assert!(!producer.try_retire_quiescent());
        producer.poison();
        let mut drain = Box::pin(producer.wait_durable_drained());
        if cancel_caller {
            drop(lookup);
            assert!(futures::poll!(drain.as_mut()).is_pending());
            release.send(()).unwrap();
        } else {
            assert!(futures::poll!(drain.as_mut()).is_pending());
            release.send(()).unwrap();
            assert_eq!(lookup.await, Err(StreamStoreError::RecoveryRequired));
        }
        drain.await;
        assert_eq!(
            producer
                .with_metadata_activity(async { panic!("retired lookup ran") })
                .await,
            Err::<(), _>(StreamStoreError::RecoveryRequired)
        );
    }
}

pub(crate) struct TestIdentity {
    pub(crate) environment_id: EnvironmentId,
    pub(crate) agent_id: AgentId,
    pub(crate) fingerprint: AgentFingerprint,
    pub(crate) invocation: StreamInvocationId,
}

pub(crate) fn identity() -> TestIdentity {
    let environment_id = EnvironmentId(Uuid::from_u128(1));
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::from_u128(2)),
        agent_id: "producer".to_string(),
    };
    let fingerprint = AgentFingerprint(Uuid::from_u128(3));
    TestIdentity {
        environment_id,
        agent_id: agent_id.clone(),
        fingerprint,
        invocation: StreamInvocationId {
            callee_environment_id: environment_id,
            callee: agent_id,
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("invocation".to_string()),
        },
    }
}

pub(crate) fn registration(
    identity: &TestIdentity,
    coordinate: StreamRegistrationCoordinate,
    source_kind: StreamSourceKind,
) -> ProducerRegistrationRequest {
    ProducerRegistrationRequest {
        entity_parent_start_index: None,
        coordinate,
        source_invocation: identity.invocation.clone(),
        component_revision: ComponentRevision::INITIAL,
        element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
        source_kind,
        session_mapping: None,
    }
}

fn root_registration(identity: &TestIdentity) -> ProducerRegistrationRequest {
    registration(
        identity,
        StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKind::MethodResult,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKind::InvocationOutput,
    )
}

pub(crate) fn attachment_key(
    identity: &TestIdentity,
    stream_id: golem_common::base_model::durable_stream::StreamId,
) -> StreamAttachmentKey {
    let consumer_environment_id = EnvironmentId(Uuid::from_u128(11));
    let consumer = AgentId {
        component_id: ComponentId(Uuid::from_u128(12)),
        agent_id: "consumer".to_string(),
    };
    let expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(13));
    let consumer_invocation = StreamInvocationId {
        callee_environment_id: consumer_environment_id,
        callee: consumer.clone(),
        callee_fingerprint: expected_consumer_fingerprint,
        idempotency_key: IdempotencyKey::new("consumer-invocation".to_string()),
    };
    StreamAttachmentKey {
        attachment_id: AttachmentId::primary(
            consumer_environment_id,
            &consumer,
            &consumer_invocation.idempotency_key,
        )
        .unwrap(),
        stream_id,
        epoch: 1,
        session_key: consumer_invocation.clone(),
        producer_environment_id: identity.environment_id,
        producer: identity.agent_id.clone(),
        expected_producer_fingerprint: identity.fingerprint,
        consumer_environment_id,
        consumer,
        expected_consumer_fingerprint,
        consumer_invocation,
    }
}

fn consumer_item_record(
    session_key: StreamSessionKey,
    stream_id: StreamId,
    source_offset: StreamOffset,
    consumer_read_ordinal: u64,
    value: Vec<u8>,
    packed_u8: bool,
) -> StreamSessionRecord {
    StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key,
        stream_id,
        source_offset,
        consumer_read_ordinal,
        value,
        packed_u8,
        recursive_handles: Vec::new(),
        recursive_mappings: Vec::new(),
    })
}

#[test]
fn consumer_journal_rejects_malformed_records_without_partial_mutation() {
    let identity = identity();
    let stream_id = StreamId(Uuid::from_u128(90));
    let key = (identity.invocation.clone(), stream_id);
    let mut index = ProducerStreamIndex::default();
    let empty_packed = consumer_item_record(
        identity.invocation.clone(),
        stream_id,
        StreamOffset::new(OplogIndex::INITIAL, 0),
        0,
        Vec::new(),
        true,
    );
    assert!(matches!(
        index.apply_consumer_journal_record(&empty_packed),
        Err(StreamStoreError::CorruptHistory(_))
    ));
    assert!(!index.consumer_journals.contains_key(&key));

    let overflowing_range = consumer_item_record(
        identity.invocation,
        stream_id,
        StreamOffset::new(OplogIndex::INITIAL, u32::MAX),
        0,
        vec![1, 2],
        true,
    );
    assert!(matches!(
        index.apply_consumer_journal_record(&overflowing_range),
        Err(StreamStoreError::CorruptHistory(_))
    ));
    assert!(!index.consumer_journals.contains_key(&key));

    index.consumer_journals.insert(
        key.clone(),
        IndexedConsumerJournal {
            next_read_ordinal: u64::MAX,
            ..Default::default()
        },
    );
    let ordinal_overflow = consumer_item_record(
        key.0.clone(),
        stream_id,
        StreamOffset::new(OplogIndex::INITIAL, 0),
        u64::MAX,
        vec![1],
        false,
    );
    assert_eq!(
        index.apply_consumer_journal_record(&ordinal_overflow),
        Err(StreamStoreError::CounterOverflow)
    );
    assert_eq!(index.consumer_journals[&key].next_read_ordinal, u64::MAX);
    assert_eq!(index.consumer_journals[&key].last_source_offset, None);
}

#[test]
fn consumer_journal_validates_terminal_order_and_duplicate_overlay_ordinal() {
    let identity = identity();
    let stream_id = StreamId(Uuid::from_u128(91));
    let attachment = attachment_key(&identity, stream_id);
    let offset = StreamOffset::new(OplogIndex::INITIAL, 0);
    let overlay = StreamSessionRecord::SourceUnavailable(
        golem_common::model::durable_stream::StreamSourceUnavailableRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key: attachment.clone(),
            source_offset: offset,
            consumer_read_ordinal: 0,
        },
    );
    let mut index = ProducerStreamIndex::default();
    index.apply_consumer_journal_record(&overlay).unwrap();
    index.apply_consumer_journal_record(&overlay).unwrap();
    let mut wrong_ordinal = overlay.clone();
    let StreamSessionRecord::SourceUnavailable(record) = &mut wrong_ordinal else {
        unreachable!()
    };
    record.consumer_read_ordinal = 1;
    assert!(matches!(
        index.apply_consumer_journal_record(&wrong_ordinal),
        Err(StreamStoreError::AttachmentConflict)
    ));

    let item = consumer_item_record(attachment.session_key, stream_id, offset, 0, vec![1], false);
    assert_eq!(
        index.apply_consumer_journal_record(&item),
        Err(StreamStoreError::ConsumerJournalAdvanced)
    );
}

#[test]
async fn consumer_source_unavailable_uses_the_warm_consumer_head_only() {
    let source_identity = identity();
    let stream_id = StreamId(Uuid::from_u128(92));
    let key = attachment_key(&source_identity, stream_id);
    let oplog = Arc::new(TestOplog::default());
    let consumer = DurableStreamStore::load(
        oplog.clone(),
        key.consumer_environment_id,
        key.consumer.clone(),
        key.expected_consumer_fingerprint,
        None,
    )
    .await
    .unwrap();
    let offset = StreamOffset::new(OplogIndex::from_u64(7), 3);
    consumer
        .append_session_record(
            None,
            StreamSessionRecord::SourceUnavailable(
                golem_common::model::durable_stream::StreamSourceUnavailableRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    source_offset: offset,
                    consumer_read_ordinal: 0,
                },
            ),
        )
        .await
        .unwrap();
    oplog.take_read_ranges();

    for _ in 0..3 {
        assert_eq!(
            consumer.consumer_source_unavailable(&key).await.unwrap(),
            Some(offset)
        );
    }
    assert!(oplog.take_read_ranges().is_empty());

    let mut wrong_identity = key;
    wrong_identity.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(999));
    assert!(matches!(
        consumer.consumer_source_unavailable(&wrong_identity).await,
        Err(StreamStoreError::InvalidAttachmentState)
    ));
}

struct FixedConsumerProbe(ConsumerAttachmentStatus);

#[async_trait]
impl StreamAttachmentConsumerProbe for FixedConsumerProbe {
    async fn status(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        Ok(self.0)
    }
}

struct CascadeConsumerProbe {
    status: ConsumerAttachmentStatus,
    inspection: std::sync::Mutex<ConsumerJournalInspection>,
    overlay_commits: AtomicU64,
}

#[async_trait]
impl StreamAttachmentConsumerProbe for CascadeConsumerProbe {
    async fn status(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        Ok(self.status)
    }

    async fn journal_inspection(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalInspection>, StreamStoreError> {
        Ok(Some(self.inspection.lock().unwrap().clone()))
    }

    async fn journal_summary(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalSummary>, StreamStoreError> {
        let inspection = self.inspection.lock().unwrap();
        Ok(Some(ConsumerJournalSummary {
            event_count: inspection.source_offsets.len() as u64,
            last_offset: inspection.source_offsets.last().copied(),
            terminal: true,
            source_unavailable: inspection.source_unavailable.is_some(),
        }))
    }

    async fn commit_source_unavailable(
        &self,
        _key: &StreamAttachmentKey,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<(), StreamStoreError> {
        let mut inspection = self.inspection.lock().unwrap();
        if inspection.source_offsets.len() as u64 != consumer_read_ordinal {
            return Err(StreamStoreError::CorruptHistory(
                "test overlay ordinal mismatch".to_string(),
            ));
        }
        match inspection.source_unavailable {
            Some(existing) if existing != source_offset => {
                return Err(StreamStoreError::CorruptHistory(
                    "test overlay conflict".to_string(),
                ));
            }
            Some(_) => return Ok(()),
            None => inspection.source_unavailable = Some(source_offset),
        }
        self.overlay_commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct FailingConsumerProbe {
    failed_stream_id: golem_common::base_model::durable_stream::StreamId,
}

#[async_trait]
impl StreamAttachmentConsumerProbe for FailingConsumerProbe {
    async fn status(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        if key.stream_id == self.failed_stream_id {
            Err(StreamStoreError::Oplog(
                "injected consumer probe failure".to_string(),
            ))
        } else {
            Ok(ConsumerAttachmentStatus::Active)
        }
    }
}

struct AdvancingCascadeProbe {
    inspection: std::sync::Mutex<ConsumerJournalInspection>,
    advanced_offset: StreamOffset,
    commits: AtomicU64,
}

#[async_trait]
impl StreamAttachmentConsumerProbe for AdvancingCascadeProbe {
    async fn status(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        Ok(ConsumerAttachmentStatus::Active)
    }

    async fn journal_inspection(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalInspection>, StreamStoreError> {
        Ok(Some(self.inspection.lock().unwrap().clone()))
    }

    async fn commit_source_unavailable(
        &self,
        _key: &StreamAttachmentKey,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<(), StreamStoreError> {
        let attempt = self.commits.fetch_add(1, Ordering::Relaxed);
        let mut inspection = self.inspection.lock().unwrap();
        if attempt == 0 {
            inspection.source_offsets.push(self.advanced_offset);
            return Err(StreamStoreError::ConsumerJournalAdvanced);
        }
        if inspection.source_offsets.len() as u64 != consumer_read_ordinal {
            return Err(StreamStoreError::ConsumerJournalAdvanced);
        }
        inspection.source_unavailable = Some(source_offset);
        Ok(())
    }
}

struct AmbiguousOverlayCommitProbe {
    inspection: std::sync::Mutex<ConsumerJournalInspection>,
    commits: AtomicU64,
}

#[async_trait]
impl StreamAttachmentConsumerProbe for AmbiguousOverlayCommitProbe {
    async fn status(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        Ok(ConsumerAttachmentStatus::Active)
    }

    async fn journal_inspection(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalInspection>, StreamStoreError> {
        Ok(Some(self.inspection.lock().unwrap().clone()))
    }

    async fn commit_source_unavailable(
        &self,
        _key: &StreamAttachmentKey,
        source_offset: StreamOffset,
        _consumer_read_ordinal: u64,
    ) -> Result<(), StreamStoreError> {
        let attempt = self.commits.fetch_add(1, Ordering::Relaxed);
        self.inspection.lock().unwrap().source_unavailable = Some(source_offset);
        if attempt == 0 {
            Err(StreamStoreError::Oplog(
                "injected response loss after overlay commit".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

async fn producer(
    oplog: Arc<TestOplog>,
    identity: &TestIdentity,
    capacity: Option<usize>,
) -> Arc<DurableStreamStore> {
    DurableStreamStore::load(
        oplog,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        capacity,
    )
    .await
    .unwrap()
}

async fn reconcile(
    producer: &DurableStreamStore,
    now_millis: u64,
    probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
) -> Result<usize, StreamStoreError> {
    producer
        .reconcile_attachments_configured(
            now_millis,
            golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RENEWAL_TARGET_MILLIS,
            golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RECONCILIATION_BATCH_SIZE,
            probe,
        )
        .await
}

#[test]
async fn delayed_stream_records_retain_registration_entity_attribution() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let entity_parent_start_index = Some(OplogIndex::from_u64(42));
    let mut request = root_registration(&identity);
    request.entity_parent_start_index = entity_parent_start_index;
    let handle = live.register(None, request).await.unwrap().value;

    oplog.add(OplogEntry::no_op(None)).await;
    live.write_items(
        None,
        handle.stream_id,
        0,
        StreamItemsPayload::PackedU8(vec![1]),
    )
    .await
    .unwrap();
    live.prepare_attachment(attachment_key(&identity, handle.stream_id), 100)
        .await
        .unwrap();
    live.end(None, handle.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap();

    let attributed = oplog
        .entries()
        .into_iter()
        .filter(|entry| {
            matches!(
                entry,
                OplogEntry::StreamRegistered { .. }
                    | OplogEntry::StreamItems { .. }
                    | OplogEntry::StreamEnd { .. }
                    | OplogEntry::StreamSession { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(attributed.len(), 4);
    assert!(
        attributed
            .iter()
            .all(|entry| { entry.entity_parent_start_index() == entity_parent_start_index })
    );

    producer(oplog, &identity, None).await;
}

#[test]
async fn item_payloads_are_loaded_only_for_the_requested_batch() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let payload = StreamItemsPayload::PackedU8(vec![7; 64]);
    let first = producer
        .write_items(None, handle.stream_id, 0, payload.clone())
        .await
        .unwrap()
        .value;
    for sequence in (64..4096).step_by(64) {
        producer
            .write_items(None, handle.stream_id, sequence, payload.clone())
            .await
            .unwrap();
    }
    for _ in 0..2050 {
        oplog.add(OplogEntry::interrupted()).await;
    }
    assert!(
        producer.index.lock().await.streams[&handle.stream_id]
            .terminal_event
            .is_none()
    );
    oplog.take_read_ranges();
    let before = oplog.point_reads.load(Ordering::Relaxed);
    let events = producer
        .read_segment(&handle, Some(first[2]), Some(first[4]))
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|event| event.offset).collect::<Vec<_>>(),
        first[3..5]
    );
    assert_eq!(oplog.point_reads.load(Ordering::Relaxed) - before, 0);
    assert!(oplog.take_read_ranges().is_empty());
    assert!(
        producer
            .write_items(None, handle.stream_id, 0, payload)
            .await
            .unwrap()
            .replayed
    );
    assert!(matches!(
        producer
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![8; 64])
            )
            .await,
        Err(StreamStoreError::EventConflict)
    ));
}

#[test]
async fn packed_retention_is_compact_and_materializes_only_a_bounded_partial_window() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let offsets = producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8((0..5000).map(|index| index as u8).collect()),
        )
        .await
        .unwrap()
        .value;

    {
        let retained = producer.committed_retention.lock().unwrap();
        assert_eq!(retained.batches.len(), 1);
        assert_eq!(retained.entries, 1);
        let super::publication::RetainedCommittedEvents::Packed {
            first_offset,
            bytes,
            ..
        } = &retained.batches.front().unwrap().events
        else {
            panic!("packed write was expanded in retention")
        };
        assert_eq!(*first_offset, offsets[0]);
        assert_eq!(bytes.len(), offsets.len());
        assert!(retained.bytes < offsets.len() * 2);
    }
    let before = oplog.point_reads.load(Ordering::Relaxed);
    let events = producer
        .read_segment(&handle, Some(offsets[9]), None)
        .await
        .unwrap();
    assert_eq!(events.len(), super::publication::STREAM_SEGMENT_MAX_EVENTS);
    assert_eq!(events[0].offset, offsets[10]);
    assert!(
        events
            .iter()
            .all(|event| event.packed_u8_batch_end == offsets.last().copied())
    );
    assert!(events.capacity() <= super::publication::STREAM_SEGMENT_MAX_EVENTS);
    assert_eq!(oplog.point_reads.load(Ordering::Relaxed), before);
}

#[test]
async fn retention_entry_budget_evicts_batches_instead_of_packed_items() {
    let identity = identity();
    let producer = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    for sequence in 0..=super::publication::COMMITTED_RETENTION_MAX_ENTRIES as u64 {
        let event = CommittedProducerStreamEvent {
            stream_id: handle.stream_id,
            producer_sequence: sequence,
            offset: StreamOffset::new(OplogIndex::from_u64(sequence + 1), 0),
            packed_u8_batch_end: None,
            terminal_author: None,
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayload::Value(vec![1]),
        };
        producer.retain_committed_events(&[event]);
    }
    let retained = producer.committed_retention.lock().unwrap();
    assert_eq!(
        retained.entries,
        super::publication::COMMITTED_RETENTION_MAX_ENTRIES
    );
    assert_eq!(
        retained.batches.len(),
        super::publication::COMMITTED_RETENTION_MAX_ENTRIES
    );
    assert!(
        retained
            .batches
            .iter()
            .all(|batch| batch.first_sequence() != 0)
    );
    assert!(retained.batches.iter().any(|batch| batch.first_sequence()
        == super::publication::COMMITTED_RETENTION_MAX_ENTRIES as u64));
}

#[test]
async fn retained_segment_returns_prefix_at_encoded_byte_bound() {
    let identity = identity();
    let producer = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    for sequence in 0..2 {
        let event = CommittedProducerStreamEvent {
            stream_id: handle.stream_id,
            producer_sequence: sequence,
            offset: StreamOffset::new(OplogIndex::from_u64(sequence + 1), 0),
            packed_u8_batch_end: None,
            terminal_author: None,
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayload::Value(vec![
                0;
                super::publication::STREAM_SEGMENT_TARGET_BYTES
                    / 2
                    + 1
            ]),
        };
        producer.retain_committed_events(&[event]);
    }
    let events = producer
        .retained_segment(handle.stream_id, None, None)
        .expect("contiguous retained prefix");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].producer_sequence, 0);
}

#[test]
async fn retention_gap_falls_back_to_authoritative_history() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let mut offsets = Vec::new();
    for sequence in 0..3 {
        offsets.extend(
            producer
                .write_items(
                    None,
                    handle.stream_id,
                    sequence,
                    StreamItemsPayload::Values(vec![vec![sequence as u8]]),
                )
                .await
                .unwrap()
                .value,
        );
    }
    {
        let mut retained = producer.committed_retention.lock().unwrap();
        let missing = retained.batches.remove(1).unwrap();
        retained.entries -= 1;
        retained.bytes -= missing.retained_bytes;
    }
    let before = oplog.point_reads.load(Ordering::Relaxed);
    let events = producer
        .read_segment(&handle, Some(offsets[0]), None)
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|event| event.offset).collect::<Vec<_>>(),
        offsets[1..]
    );
    assert!(oplog.point_reads.load(Ordering::Relaxed) > before);
}

#[test]
async fn cursor_validation_point_reads_without_historical_event_cache() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let offsets = producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![1, 2, 3]),
        )
        .await
        .unwrap()
        .value;
    for _ in 0..2050 {
        oplog.add(OplogEntry::interrupted()).await;
    }
    producer
        .end(None, handle.stream_id, 3, StreamEndResult::Ok)
        .await
        .unwrap();
    let high_water = producer.input_high_water(handle.stream_id).await.unwrap();
    {
        let mut index = producer.index.lock().await;
        let stream = index.streams.get_mut(&handle.stream_id).unwrap();
        stream.terminal_event = None;
        stream.batches.clear();
    }
    assert_eq!(
        producer.input_high_water(handle.stream_id).await.unwrap(),
        high_water
    );
    oplog.take_read_ranges();
    let before = oplog.point_reads.load(Ordering::Relaxed);
    producer
        .validate_cursor(handle.stream_id, Some(offsets[1]))
        .await
        .unwrap();
    assert_eq!(oplog.point_reads.load(Ordering::Relaxed) - before, 0);
    assert!(oplog.take_read_ranges().is_empty());
    assert!(
        producer
            .validate_cursor(
                handle.stream_id,
                Some(StreamOffset::new(offsets[0].producer_oplog_index(), 3)),
            )
            .await
            .is_err()
    );
    assert!(
        producer
            .validate_cursor(
                handle.stream_id,
                Some(StreamOffset::new(OplogIndex::NONE, 0)),
            )
            .await
            .is_err()
    );
}

#[test]
async fn attachment_lifecycle_is_idempotent_fenced_and_rebuildable() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let key = attachment_key(&identity, handle.stream_id);

    let prepared = live.prepare_attachment(key.clone(), 100).await.unwrap();
    assert!(!prepared.replayed);
    assert_eq!(prepared.value.state, StreamAttachmentState::Prepared);
    assert_eq!(
        prepared.value.lease_expires_at_millis,
        Some(100 + STREAM_ATTACHMENT_LEASE_TTL_MILLIS)
    );
    assert!(
        live.prepare_attachment(key.clone(), 101)
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(oplog.committed_length(), 2);
    assert_eq!(
        live.read_attached_segment(&key, &handle, 102, None, None)
            .await,
        Err(StreamStoreError::InvalidAttachmentState)
    );

    let mut malformed = key.clone();
    malformed.epoch = 0;
    assert_eq!(
        live.activate_attachment(malformed, 110).await,
        Err(StreamStoreError::CorruptHistory(
            "unsupported or malformed durable attachment record".to_string()
        ))
    );
    let mut future = key.clone();
    future.epoch = 2;
    assert_eq!(
        live.activate_attachment(future, 110).await,
        Err(StreamStoreError::InvalidEpoch {
            current: 1,
            actual: 2,
        })
    );

    assert!(
        !live
            .activate_attachment(key.clone(), 120)
            .await
            .unwrap()
            .replayed
    );
    let after_activate = oplog.committed_length();
    assert!(
        live.activate_attachment(key.clone(), 120)
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(oplog.committed_length(), after_activate);
    assert!(
        live.read_attached_segment(&key, &handle, 121, None, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        live.read_attached_segment(
            &key,
            &handle,
            120 + STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
            None,
            None,
        )
        .await,
        Err(StreamStoreError::LeaseExpired)
    );
    assert!(
        !live
            .renew_attachment(key.clone(), 130)
            .await
            .unwrap()
            .replayed
    );
    let after_renew = oplog.committed_length();
    assert!(
        live.renew_attachment(key.clone(), 130)
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(oplog.committed_length(), after_renew);
    assert!(
        !live
            .finalize_attachment(
                key.clone(),
                StreamAttachmentFinalizationReason::ConsumerFinalized,
                140,
            )
            .await
            .unwrap()
            .replayed
    );
    let after_finalize = oplog.committed_length();
    assert!(
        live.finalize_attachment(
            key.clone(),
            StreamAttachmentFinalizationReason::ConsumerFinalized,
            140,
        )
        .await
        .unwrap()
        .replayed
    );
    assert_eq!(oplog.committed_length(), after_finalize);
    live.commit_deletion_barrier(1_000, true).await.unwrap();
    drop(live);

    let restarted = producer(oplog, &identity, None).await;
    let attachments = restarted.inspect_attachments().await;
    let [view] = attachments.as_slice() else {
        panic!("restarted producer must rebuild exactly one attachment")
    };
    assert_eq!(view.key, key);
    assert_eq!(
        view.state,
        StreamAttachmentState::Finalized(StreamAttachmentFinalizationReason::ConsumerFinalized)
    );
}

#[test]
async fn attachment_slots_are_isolated_by_consumer_identity() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let first = attachment_key(&identity, handle.stream_id);
    let mut second = first.clone();
    second.consumer_environment_id = EnvironmentId(Uuid::from_u128(21));
    second.consumer = AgentId {
        component_id: ComponentId(Uuid::from_u128(22)),
        agent_id: "second-consumer".to_string(),
    };
    second.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(23));
    second.consumer_invocation.callee_environment_id = second.consumer_environment_id;
    second.consumer_invocation.callee = second.consumer.clone();
    second.consumer_invocation.callee_fingerprint = second.expected_consumer_fingerprint;

    for key in [&first, &second] {
        assert!(
            !live
                .prepare_attachment(key.clone(), 100)
                .await
                .unwrap()
                .replayed
        );
        assert!(
            live.prepare_attachment(key.clone(), 101)
                .await
                .unwrap()
                .replayed
        );
        assert!(
            !live
                .activate_attachment(key.clone(), 110)
                .await
                .unwrap()
                .replayed
        );
        assert!(
            live.activate_attachment(key.clone(), 111)
                .await
                .unwrap()
                .replayed
        );
    }

    let mut invalid_identity = first.clone();
    invalid_identity.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(99));
    invalid_identity.consumer_invocation.callee_fingerprint =
        invalid_identity.expected_consumer_fingerprint;
    assert_eq!(
        live.prepare_attachment(invalid_identity, 120).await,
        Err(StreamStoreError::InvalidAttachmentState)
    );
    let mut invalid_epoch = second.clone();
    invalid_epoch.epoch = 2;
    assert_eq!(
        live.activate_attachment(invalid_epoch, 120).await,
        Err(StreamStoreError::InvalidEpoch {
            current: 1,
            actual: 2,
        })
    );

    live.finalize_attachment(
        first.clone(),
        StreamAttachmentFinalizationReason::ConsumerFinalized,
        130,
    )
    .await
    .unwrap();
    assert!(
        live.read_attached_segment(&second, &handle, 131, None, None)
            .await
            .unwrap()
            .is_empty()
    );
    drop(live);

    let restarted = producer(oplog, &identity, None).await;
    let attachments = restarted.inspect_attachments().await;
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].key, first);
    assert_eq!(
        attachments[0].state,
        StreamAttachmentState::Finalized(StreamAttachmentFinalizationReason::ConsumerFinalized)
    );
    assert_eq!(attachments[1].key, second);
    assert_eq!(attachments[1].state, StreamAttachmentState::Active);
    assert!(
        restarted
            .activate_attachment(second, 140)
            .await
            .unwrap()
            .replayed
    );
}

#[test]
async fn active_attachment_count_spans_distinct_consumer_slots() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog, &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let mut first = attachment_key(&identity, handle.stream_id);
    first.session_key = identity.invocation.clone();
    first.attachment_id = AttachmentId::primary(
        first.session_key.callee_environment_id,
        &first.session_key.callee,
        &first.session_key.idempotency_key,
    )
    .unwrap();
    let mut second = first.clone();
    second.consumer_environment_id = EnvironmentId(Uuid::from_u128(21));
    second.consumer = AgentId {
        component_id: ComponentId(Uuid::from_u128(22)),
        agent_id: "second-consumer".to_string(),
    };
    second.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(23));
    second.consumer_invocation.callee_environment_id = second.consumer_environment_id;
    second.consumer_invocation.callee = second.consumer.clone();
    second.consumer_invocation.callee_fingerprint = second.expected_consumer_fingerprint;

    live.prepare_attachment(first.clone(), 100).await.unwrap();
    assert!(
        !live
            .has_active_attachment(&first.session_key, &handle)
            .await
            .unwrap()
    );
    live.activate_attachment(first.clone(), 110).await.unwrap();
    live.prepare_attachment(second.clone(), 100).await.unwrap();
    live.activate_attachment(second.clone(), 110).await.unwrap();
    assert!(
        live.has_active_attachment(&first.session_key, &handle)
            .await
            .unwrap()
    );
    live.finalize_attachment(
        first.clone(),
        StreamAttachmentFinalizationReason::ConsumerFinalized,
        120,
    )
    .await
    .unwrap();
    assert!(
        live.has_active_attachment(&first.session_key, &handle)
            .await
            .unwrap()
    );
    live.finalize_attachment(
        second,
        StreamAttachmentFinalizationReason::ConsumerFinalized,
        120,
    )
    .await
    .unwrap();
    assert!(
        !live
            .has_active_attachment(&first.session_key, &handle)
            .await
            .unwrap()
    );
}

#[test]
async fn replacing_a_source_cancellation_fences_the_old_drain_without_losing_the_new_one() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let old = CancellationToken::new();
    let old_id = producer.register_source_cancellation(handle.stream_id, old.clone());
    let current = CancellationToken::new();
    let current_id = producer.register_source_cancellation(handle.stream_id, current.clone());

    assert!(old.is_cancelled());
    assert!(!current.is_cancelled());
    producer.unregister_source_cancellation(handle.stream_id, old_id);
    producer
        .cancel_open(
            None,
            handle.stream_id,
            StreamCancelRole::OutputConsumer,
            StreamCancelReason::GuestDrop,
            None,
        )
        .await
        .unwrap();
    assert!(current.is_cancelled());
    producer.unregister_source_cancellation(handle.stream_id, current_id);
}

#[test]
async fn producer_rejects_handles_with_altered_non_identity_metadata_before_attachment() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let registration_length = oplog.current_oplog_index().await;

    let mut altered_source = handle.clone();
    altered_source.source_invocation.idempotency_key =
        IdempotencyKey::new("altered-source".to_string());
    let mut altered_revision = handle.clone();
    altered_revision.component_revision = ComponentRevision::new(2).unwrap();
    let mut altered_schema = handle;
    altered_schema.element_schema_fingerprint = SchemaFingerprintV1([8; 32]);

    for altered in [altered_source, altered_revision, altered_schema] {
        assert_eq!(
            live.validate_handle(&altered).await,
            Err(StreamStoreError::InvalidHandle)
        );
    }
    assert_eq!(oplog.current_oplog_index().await, registration_length);
    assert!(live.inspect_attachments().await.is_empty());
}

#[test]
async fn deletion_is_fail_closed_for_prepared_active_and_expired_references() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog, &identity, None).await;
    let first = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let key = attachment_key(&identity, first.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    assert!(matches!(
        live.commit_deletion_barrier(1_000, true).await,
        Err(StreamStoreError::DeletionBlocked(ref dependents))
            if dependents == std::slice::from_ref(&key)
    ));
    live.activate_attachment(key.clone(), 110).await.unwrap();
    assert_eq!(
        live.read_attached_segment(
            &key,
            &first,
            110 + STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
            None,
            None,
        )
        .await,
        Err(StreamStoreError::LeaseExpired)
    );
    assert!(matches!(
        live.commit_deletion_barrier(1_000, true).await,
        Err(StreamStoreError::DeletionBlocked(ref dependents))
            if dependents == std::slice::from_ref(&key)
    ));
    live.finalize_attachment(
        key,
        StreamAttachmentFinalizationReason::ConsumerFinalized,
        200,
    )
    .await
    .unwrap();
    live.commit_deletion_barrier(1_000, true).await.unwrap();

    let second_session = StreamInvocationId {
        idempotency_key: IdempotencyKey::new("second".to_string()),
        ..identity.invocation.clone()
    };
    assert_eq!(
        live.register(
            None,
            ProducerRegistrationRequest {
                coordinate: StreamRegistrationCoordinate::Root {
                    invocation_id: second_session.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: second_session,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKind::InvocationOutput,
                session_mapping: None,
                entity_parent_start_index: None,
            }
        )
        .await,
        Err(StreamStoreError::ProducerDeleting)
    );
}

#[test]
async fn deletion_gate_and_attachment_prepare_have_one_linearization_order() {
    for _ in 0..16 {
        let identity = identity();
        let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let handle = live
            .register(None, root_registration(&identity))
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);
        let barrier = Arc::new(Barrier::new(3));
        let deletion = {
            let live = live.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                live.commit_deletion_barrier(1_000, true).await
            })
        };
        let prepare = {
            let live = live.clone();
            let barrier = barrier.clone();
            let key = key.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                live.prepare_attachment(key, 100).await
            })
        };
        barrier.wait().await;

        match (deletion.await.unwrap(), prepare.await.unwrap()) {
            (Ok(()), Err(StreamStoreError::ProducerDeleting)) => {}
            (Err(StreamStoreError::DeletionBlocked(dependents)), Ok(prepared)) => {
                assert_eq!(dependents, vec![key]);
                assert_eq!(prepared.value.state, StreamAttachmentState::Prepared);
            }
            outcome => panic!("deletion/prepare race was not linearized: {outcome:?}"),
        }
    }
}

#[test]
async fn deleting_producer_restarts_without_renewing_before_cascade_retry() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let first_offset = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7]),
        )
        .await
        .unwrap()
        .value[0];
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 110).await.unwrap();
    live.commit_deletion_barrier(200, false).await.unwrap();
    let after_barrier = oplog.committed_length();
    live.commit_deletion_barrier(200, false).await.unwrap();
    assert_eq!(oplog.committed_length(), after_barrier);
    drop(live);

    let restarted = producer(oplog, &identity, None).await;
    let probe = CascadeConsumerProbe {
        status: ConsumerAttachmentStatus::Active,
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: Vec::new(),
            source_unavailable: None,
        }),
        overlay_commits: AtomicU64::new(0),
    };
    assert_eq!(
        restarted
            .reconcile_attachments_configured(1_000, 1, 256, &probe)
            .await
            .unwrap(),
        0
    );
    restarted.cascade_deletion(1_001, &probe).await.unwrap();
    assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
    assert_eq!(
        restarted
            .deletion_diagnostics()
            .await
            .unwrap()
            .cascade_completed,
        vec![(
            key,
            StreamCascadeDependentResult::SourceUnavailable {
                first_unjournaled_offset: first_offset,
            },
        )]
    );
}

#[test]
#[timeout("30s")]
async fn deletion_cascade_does_not_wait_for_a_stalled_live_reader() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, Some(1)).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let mut reader = live
        .bus(handle.stream_id)
        .unwrap()
        .subscribe()
        .await
        .unwrap();
    let first_offset = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7]),
        )
        .await
        .unwrap()
        .value[0];
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 110).await.unwrap();
    let probe = CascadeConsumerProbe {
        status: ConsumerAttachmentStatus::Active,
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: Vec::new(),
            source_unavailable: None,
        }),
        overlay_commits: AtomicU64::new(0),
    };

    tokio::time::timeout(Duration::from_secs(5), live.cascade_deletion(200, &probe))
        .await
        .expect("deletion waited for live delivery instead of durability")
        .unwrap();
    assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
    assert_eq!(
        probe.inspection.lock().unwrap().source_unavailable,
        Some(first_offset)
    );
    let restarted = producer(oplog, &identity, None).await;
    assert_eq!(
        restarted
            .deletion_diagnostics()
            .await
            .unwrap()
            .cascade_completed,
        vec![(
            key,
            StreamCascadeDependentResult::SourceUnavailable {
                first_unjournaled_offset: first_offset,
            }
        )]
    );
    assert_eq!(reader.recv().await.unwrap().offset, first_offset);
    let terminal = reader.recv().await.unwrap();
    assert!(terminal.offset > first_offset);
    assert!(terminal.payload.is_terminal());
}

#[test]
async fn cascade_is_durable_idempotent_and_overlays_the_first_unjournaled_position() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let item_offset = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7]),
        )
        .await
        .unwrap()
        .value[0];
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 110).await.unwrap();
    let probe = CascadeConsumerProbe {
        status: ConsumerAttachmentStatus::Active,
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: Vec::new(),
            source_unavailable: None,
        }),
        overlay_commits: AtomicU64::new(0),
    };

    live.cascade_deletion(200, &probe).await.unwrap();
    assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
    assert_eq!(
        probe.inspection.lock().unwrap().source_unavailable,
        Some(item_offset)
    );
    let committed_length = oplog.committed_length();
    live.cascade_deletion(201, &probe).await.unwrap();
    assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
    assert_eq!(oplog.committed_length(), committed_length);
    assert_eq!(
        live.write_items(
            None,
            handle.stream_id,
            1,
            StreamItemsPayload::PackedU8(vec![8])
        )
        .await,
        Err(StreamStoreError::ProducerDeleting)
    );

    let diagnostics = live.deletion_diagnostics().await.unwrap();
    assert!(diagnostics.deleting);
    assert_eq!(diagnostics.attachments.len(), 1);
    assert_eq!(
        diagnostics.cascade_completed,
        vec![(
            key.clone(),
            StreamCascadeDependentResult::SourceUnavailable {
                first_unjournaled_offset: item_offset,
            },
        )]
    );
    drop(live);
    let restarted = producer(oplog, &identity, None).await;
    assert_eq!(restarted.deletion_diagnostics().await.unwrap(), diagnostics);
}

#[test]
async fn cascade_retries_when_the_consumer_journal_advances_before_overlay_commit() {
    let identity = identity();
    let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let offsets = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7, 8]),
        )
        .await
        .unwrap()
        .value;
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 110).await.unwrap();
    let probe = AdvancingCascadeProbe {
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: Vec::new(),
            source_unavailable: None,
        }),
        advanced_offset: offsets[0],
        commits: AtomicU64::new(0),
    };

    assert_eq!(
        live.cascade_deletion(200, &probe).await,
        Err(StreamStoreError::ConsumerJournalAdvanced)
    );
    assert!(
        live.deletion_diagnostics()
            .await
            .unwrap()
            .cascade_completed
            .is_empty()
    );
    live.cascade_deletion(201, &probe).await.unwrap();
    assert_eq!(probe.commits.load(Ordering::Relaxed), 2);
    assert_eq!(
        probe.inspection.lock().unwrap().source_unavailable,
        Some(offsets[1])
    );
    assert_eq!(
        live.deletion_diagnostics().await.unwrap().cascade_completed,
        vec![(
            key,
            StreamCascadeDependentResult::SourceUnavailable {
                first_unjournaled_offset: offsets[1],
            },
        )]
    );
}

#[test]
async fn cascade_retries_after_overlay_commit_before_outbox_commit() {
    let identity = identity();
    let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let offset = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7]),
        )
        .await
        .unwrap()
        .value[0];
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 110).await.unwrap();
    let probe = AmbiguousOverlayCommitProbe {
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: Vec::new(),
            source_unavailable: None,
        }),
        commits: AtomicU64::new(0),
    };

    assert!(matches!(
        live.cascade_deletion(200, &probe).await,
        Err(StreamStoreError::Oplog(_))
    ));
    assert_eq!(
        probe.inspection.lock().unwrap().source_unavailable,
        Some(offset)
    );
    assert!(
        live.deletion_diagnostics()
            .await
            .unwrap()
            .cascade_completed
            .is_empty()
    );
    live.cascade_deletion(201, &probe).await.unwrap();
    assert_eq!(probe.commits.load(Ordering::Relaxed), 1);
    assert_eq!(
        live.deletion_diagnostics().await.unwrap().cascade_completed,
        vec![(
            key,
            StreamCascadeDependentResult::SourceUnavailable {
                first_unjournaled_offset: offset,
            },
        )]
    );
}

#[test]
async fn source_unavailable_and_consumer_journal_append_are_serialized() {
    let source_identity = identity();
    let source = producer(Arc::new(TestOplog::default()), &source_identity, None).await;
    let handle = source
        .register(None, root_registration(&source_identity))
        .await
        .unwrap()
        .value;
    let offsets = source
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7, 8]),
        )
        .await
        .unwrap()
        .value;
    let key = attachment_key(&source_identity, handle.stream_id);
    let consumer_identity = TestIdentity {
        environment_id: key.consumer_environment_id,
        agent_id: key.consumer.clone(),
        fingerprint: key.expected_consumer_fingerprint,
        invocation: key.consumer_invocation.clone(),
    };
    let consumer = producer(Arc::new(TestOplog::default()), &consumer_identity, None).await;
    let first_item = StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: key.session_key.clone(),
        stream_id: key.stream_id,
        source_offset: offsets[0],
        consumer_read_ordinal: 0,
        value: vec![7],
        packed_u8: true,
        recursive_handles: Vec::new(),
        recursive_mappings: Vec::new(),
    });
    let barrier = Arc::new(Barrier::new(3));
    let overlay_task = {
        let consumer = consumer.clone();
        let barrier = barrier.clone();
        let key = key.clone();
        let first_offset = offsets[0];
        tokio::spawn(async move {
            barrier.wait().await;
            consumer
                .commit_source_unavailable_overlay(None, key, first_offset, 0)
                .await
        })
    };
    let item_task = {
        let consumer = consumer.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            consumer.append_session_record(None, first_item).await
        })
    };
    barrier.wait().await;
    let overlay_result = overlay_task.await.unwrap();
    let item_result = item_task.await.unwrap();

    match (overlay_result, item_result) {
        (Ok(false), Err(StreamStoreError::ConsumerJournalAdvanced)) => {
            assert!(
                consumer
                    .commit_source_unavailable_overlay(None, key.clone(), offsets[0], 0)
                    .await
                    .unwrap()
            );
        }
        (Err(StreamStoreError::ConsumerJournalAdvanced), Ok(())) => {
            assert!(
                !consumer
                    .commit_source_unavailable_overlay(None, key.clone(), offsets[1], 1)
                    .await
                    .unwrap()
            );
        }
        results => panic!("journal/overlay race was not linearized: {results:?}"),
    }

    assert_eq!(
        consumer
            .append_session_record(
                None,
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key.session_key,
                    stream_id: key.stream_id,
                    source_offset: offsets[1],
                    consumer_read_ordinal: 1,
                    value: vec![8],
                    packed_u8: true,
                    recursive_handles: Vec::new(),
                    recursive_mappings: Vec::new(),
                },)
            )
            .await,
        Err(StreamStoreError::ConsumerJournalAdvanced)
    );
}

#[test]
async fn consumer_deleting_intent_fences_prepared_and_activated_topology() {
    let source_identity = identity();
    let source = producer(Arc::new(TestOplog::default()), &source_identity, None).await;
    let handle = source
        .register(None, root_registration(&source_identity))
        .await
        .unwrap()
        .value;
    let key = attachment_key(&source_identity, handle.stream_id);
    let consumer_identity = TestIdentity {
        environment_id: key.consumer_environment_id,
        agent_id: key.consumer.clone(),
        fingerprint: key.expected_consumer_fingerprint,
        invocation: key.consumer_invocation.clone(),
    };
    let consumer = producer(Arc::new(TestOplog::default()), &consumer_identity, None).await;
    consumer
        .append_session_record(
            None,
            StreamSessionRecord::ConsumerDeleting(StreamConsumerDeletingRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                consumer_environment_id: consumer_identity.environment_id,
                consumer: consumer_identity.agent_id,
                consumer_fingerprint: consumer_identity.fingerprint,
                deleting_at_millis: 100,
            }),
        )
        .await
        .unwrap();
    let mapping = StreamSessionMappingRecord {
        transport_stream_id: 0,
        handle,
        role: SessionStreamRole::Input,
    };

    assert_eq!(
        consumer
            .append_session_record(
                None,
                StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key.session_key.clone(),
                    attachment: key.clone(),
                    mapping: mapping.clone(),
                },)
            )
            .await,
        Err(StreamStoreError::ConsumerDeleting)
    );
    assert_eq!(
        consumer
            .append_session_record(
                None,
                StreamSessionRecord::TopologyActivated(StreamTopologyActivatedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key.session_key.clone(),
                    attachment: key,
                    mapping,
                },)
            )
            .await,
        Err(StreamStoreError::ConsumerDeleting)
    );
}

#[test]
async fn complete_value_journal_releases_dependency_only_after_the_source_terminal() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog, &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let item_offset = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7]),
        )
        .await
        .unwrap()
        .value[0];
    let terminal_offset = live
        .end(None, handle.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap()
        .value;
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 110).await.unwrap();
    let incomplete = CascadeConsumerProbe {
        status: ConsumerAttachmentStatus::Active,
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: vec![item_offset],
            source_unavailable: None,
        }),
        overlay_commits: AtomicU64::new(0),
    };
    assert_eq!(reconcile(&live, 130, &incomplete).await.unwrap(), 0);
    assert!(matches!(
        live.inspect_attachments().await[0].state,
        StreamAttachmentState::Active
    ));

    let complete = CascadeConsumerProbe {
        status: ConsumerAttachmentStatus::Active,
        inspection: std::sync::Mutex::new(ConsumerJournalInspection {
            source_offsets: vec![item_offset, terminal_offset],
            source_unavailable: None,
        }),
        overlay_commits: AtomicU64::new(0),
    };
    assert_eq!(reconcile(&live, 150, &complete).await.unwrap(), 1);
    assert_eq!(
        live.inspect_attachments().await[0].state,
        StreamAttachmentState::Finalized(StreamAttachmentFinalizationReason::ConsumerFinalized)
    );
    live.commit_deletion_barrier(1_000, true).await.unwrap();
}

#[test]
async fn reconciliation_adopts_rolls_back_and_fences_recreated_consumers() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog, &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();

    assert_eq!(
        reconcile(
            &live,
            110,
            &FixedConsumerProbe(ConsumerAttachmentStatus::Active),
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        live.inspect_attachments().await[0].state,
        StreamAttachmentState::Active
    );
    assert_eq!(
        reconcile(
            &live,
            120,
            &FixedConsumerProbe(ConsumerAttachmentStatus::IncarnationMismatch),
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        live.inspect_attachments().await[0].state,
        StreamAttachmentState::Finalized(
            StreamAttachmentFinalizationReason::ConsumerIncarnationChanged
        )
    );

    let second_session = StreamInvocationId {
        idempotency_key: IdempotencyKey::new("abandoned".to_string()),
        ..identity.invocation.clone()
    };
    let second = live
        .register(
            None,
            ProducerRegistrationRequest {
                coordinate: StreamRegistrationCoordinate::Root {
                    invocation_id: second_session.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: second_session,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKind::InvocationOutput,
                session_mapping: None,
                entity_parent_start_index: None,
            },
        )
        .await
        .unwrap()
        .value;
    let abandoned = attachment_key(&identity, second.stream_id);
    live.prepare_attachment(abandoned.clone(), 200)
        .await
        .unwrap();
    assert_eq!(
        reconcile(
            &live,
            200 + STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS - 1,
            &FixedConsumerProbe(ConsumerAttachmentStatus::Missing),
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        reconcile(
            &live,
            200 + STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS,
            &FixedConsumerProbe(ConsumerAttachmentStatus::Missing),
        )
        .await
        .unwrap(),
        1
    );
    assert!(matches!(
        live.attachment_view(&abandoned).await.unwrap().state,
        StreamAttachmentState::Finalized(StreamAttachmentFinalizationReason::PrepareAbandoned)
    ));
}

#[test]
async fn reconciliation_processes_every_attachment_beyond_one_batch() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog, &identity, None).await;
    let attachment_count =
        golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RECONCILIATION_BATCH_SIZE + 1;
    for index in 0..attachment_count {
        let handle = live
            .register(
                None,
                ProducerRegistrationRequest {
                    coordinate: StreamRegistrationCoordinate::Root {
                        invocation_id: identity.invocation.clone(),
                        root_kind: StreamRootKind::MethodResult,
                        recursive_value_path: vec![StreamValuePathStep::ListElement(index as u32)],
                    },
                    ..root_registration(&identity)
                },
            )
            .await
            .unwrap()
            .value;
        live.prepare_attachment(attachment_key(&identity, handle.stream_id), 100)
            .await
            .unwrap();
    }

    let probe = FixedConsumerProbe(ConsumerAttachmentStatus::Active);
    let first_batch = reconcile(&live, 110, &probe).await.unwrap();
    let second_batch = reconcile(&live, 110, &probe).await.unwrap();
    assert_eq!(first_batch + second_batch, attachment_count);
    assert!(
        live.inspect_attachments()
            .await
            .iter()
            .all(|attachment| attachment.state == StreamAttachmentState::Active)
    );
}

#[test]
async fn reconciliation_continues_after_an_earlier_probe_failure() {
    let identity = identity();
    let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let mut keys = Vec::new();
    for index in 0..2 {
        let handle = live
            .register(
                None,
                ProducerRegistrationRequest {
                    coordinate: StreamRegistrationCoordinate::Root {
                        invocation_id: identity.invocation.clone(),
                        root_kind: StreamRootKind::MethodResult,
                        recursive_value_path: vec![StreamValuePathStep::ListElement(index)],
                    },
                    ..root_registration(&identity)
                },
            )
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        keys.push(key);
    }
    keys.sort_by_key(|key| (key.stream_id, key.attachment_id, key.epoch));

    assert!(
        reconcile(
            &live,
            110,
            &FailingConsumerProbe {
                failed_stream_id: keys[0].stream_id,
            },
        )
        .await
        .is_err()
    );
    assert_eq!(
        live.attachment_view(&keys[0]).await.unwrap().state,
        StreamAttachmentState::Prepared
    );
    assert_eq!(
        live.attachment_view(&keys[1]).await.unwrap().state,
        StreamAttachmentState::Active
    );
}

#[test]
async fn session_record_commit_folds_a_pending_invocation_added_immediately_before_it() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let committed_batches = Arc::new(Mutex::new(Vec::<Vec<OplogEntry>>::new()));
    let oplog_for_commit = oplog.clone();
    let batches_for_commit = committed_batches.clone();
    let commit: DurableStreamCommit = Arc::new(move |committed| {
        let oplog = oplog_for_commit.clone();
        let batches = batches_for_commit.clone();
        Box::pin(async move {
            let committed_entries = oplog.commit(CommitLevel::Always).await;
            batches
                .lock()
                .unwrap()
                .push(committed_entries.into_values().collect());
            if let Some(committed) = committed {
                let _ = committed.send(());
            }
        })
    });
    let producer = DurableStreamStore::load_with_commit(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let stream_id = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value
        .stream_id;
    committed_batches.lock().unwrap().clear();

    oplog
        .add(OplogEntry::pending_agent_invocation(
            IdempotencyKey::new("pending-before-consumer-journal".to_string()),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await;
    producer
        .append_session_record(
            None,
            StreamSessionRecord::ConsumerTerminal(
                golem_common::model::durable_stream::StreamConsumerTerminalRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation,
                    stream_id,
                    source_offset: StreamOffset::new(OplogIndex::INITIAL, 0),
                    consumer_read_ordinal: 0,
                    terminal: golem_common::model::durable_stream::StreamConsumerTerminal::End(
                        StreamEndResult::Ok,
                    ),
                },
            ),
        )
        .await
        .unwrap();

    let batches = committed_batches.lock().unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 2);
    assert!(matches!(
        batches[0][0],
        OplogEntry::PendingAgentInvocation { .. }
    ));
    assert!(matches!(batches[0][1], OplogEntry::StreamSession { .. }));
}

#[test]
async fn producer_journal_restarts_and_replays_without_appending() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live_producer = producer(oplog.clone(), &identity, None).await;
    let registered = live_producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    assert!(!registered.replayed);
    let stream_id = registered.value.stream_id;
    assert_eq!(
        live_producer.input_high_water(stream_id).await.unwrap(),
        None
    );

    let written = live_producer
        .write_items(
            None,
            stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![10, 11]),
        )
        .await
        .unwrap();
    assert_eq!(written.value.len(), 2);
    assert_eq!(
        written.value[0].producer_oplog_index(),
        OplogIndex::from_u64(2)
    );
    assert_eq!(written.value[0].sub_index(), 0);
    assert_eq!(written.value[1].sub_index(), 1);
    assert_eq!(
        live_producer.input_high_water(stream_id).await.unwrap(),
        Some(InputStreamHighWater {
            highest_contiguous_sequence: 1,
            resulting_offset: written.value[1],
            terminal: false,
        })
    );
    let terminal = live_producer
        .end(None, stream_id, 2, StreamEndResult::Ok)
        .await
        .unwrap();
    assert_eq!(
        terminal.value.producer_oplog_index(),
        OplogIndex::from_u64(3)
    );
    assert_eq!(
        live_producer.input_high_water(stream_id).await.unwrap(),
        Some(InputStreamHighWater {
            highest_contiguous_sequence: 2,
            resulting_offset: terminal.value,
            terminal: true,
        })
    );
    assert_eq!(oplog.committed_length(), 3);

    drop(live_producer);
    let restarted = producer(oplog.clone(), &identity, None).await;
    let replayed_registration = restarted
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    assert!(replayed_registration.replayed);
    assert_eq!(replayed_registration.value, registered.value);
    assert!(
        restarted
            .write_items(
                None,
                stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![10, 11])
            )
            .await
            .unwrap()
            .replayed
    );
    assert!(
        restarted
            .end(None, stream_id, 2, StreamEndResult::Ok)
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(
        restarted.input_high_water(stream_id).await.unwrap(),
        Some(InputStreamHighWater {
            highest_contiguous_sequence: 2,
            resulting_offset: terminal.value,
            terminal: true,
        })
    );
    assert_eq!(oplog.committed_length(), 3);

    let mut reader = restarted
        .catch_up(registered.value, Some(written.value[0]))
        .await
        .unwrap();
    assert_eq!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::PackedU8(11)
    );
    assert_eq!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    );
    assert!(reader.next().await.unwrap().is_none());
}

#[test]
#[test_r::timeout("30s")]
async fn closed_export_reports_terminal_state_before_final_page() {
    use golem_common::model::durable_stream::StreamHandleReadRequest;
    let identity = identity();
    let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let offsets = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![3, 9, 17, 41]),
        )
        .await
        .unwrap()
        .value;
    live.end(None, handle.stream_id, 4, StreamEndResult::Ok)
        .await
        .unwrap();
    let read = live
        .read_by_handle(StreamHandleReadRequest {
            handle,
            after: None,
            max_items: 2,
            max_bytes: 10,
            wait_millis: 0,
        })
        .await
        .unwrap();
    assert!(read.closed);
    assert!(!read.cancelled);
    assert_eq!(read.events.len(), 2);
    assert_eq!(read.next_offset, Some(offsets[1]));
    assert!(read.next_offset < read.head_offset);
}

#[test]
#[test_r::timeout("30s")]
async fn blocked_terminal_publication_wakes_export_reader_after_durable_commit() {
    use golem_common::model::durable_stream::StreamHandleReadRequest;
    let identity = identity();
    let live = producer(Arc::new(TestOplog::default()), &identity, Some(1)).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let bus = live.stream_bus(handle.stream_id).await.unwrap();
    let mut reader = bus.subscribe().await.unwrap();
    let offset = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![3]),
        )
        .await
        .unwrap()
        .value[0];

    let export = live.read_by_handle(StreamHandleReadRequest {
        handle: handle.clone(),
        after: Some(offset),
        max_items: 2,
        max_bytes: 10,
        wait_millis: 5_000,
    });
    tokio::pin!(export);
    assert!(futures::poll!(&mut export).is_pending());

    let terminal = tokio::spawn({
        let live = live.clone();
        async move {
            live.end(None, handle.stream_id, 1, StreamEndResult::Ok)
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let head = live.stream_head(&handle).await.unwrap();
            if head.closed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal commit did not finish");
    assert!(
        !terminal.is_finished(),
        "publication must remain backpressured"
    );

    let read = tokio::time::timeout(std::time::Duration::from_millis(100), export)
        .await
        .expect("durably committed terminal must wake the export reader")
        .unwrap();
    assert!(read.closed);
    assert_eq!(read.events.len(), 1);
    reader.recv().await.unwrap();
    terminal.await.unwrap().unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn unattached_slot_reads_paginate_and_wake_both_readers() {
    use golem_common::model::durable_stream::StreamHandleReadRequest;
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let key = attachment_key(&identity, handle.stream_id);
    live.prepare_attachment(key.clone(), 100).await.unwrap();
    live.activate_attachment(key.clone(), 101).await.unwrap();
    let offsets = live
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![3, 9, 17]),
        )
        .await
        .unwrap()
        .value;
    let request = StreamHandleReadRequest {
        handle: handle.clone(),
        after: None,
        max_items: 2,
        max_bytes: 10,
        wait_millis: 0,
    };
    let first = live.read_by_handle(request.clone()).await.unwrap();
    assert_eq!(
        first
            .events
            .iter()
            .map(|event| event.payload.clone())
            .collect::<Vec<_>>(),
        vec![
            CommittedProducerStreamEventPayload::PackedU8(3),
            CommittedProducerStreamEventPayload::PackedU8(9)
        ]
    );
    assert_eq!(first.next_offset, Some(offsets[1]));
    assert_eq!(first.head_offset, Some(offsets[2]));
    let byte_limited = live
        .read_by_handle(StreamHandleReadRequest {
            max_items: 20,
            max_bytes: 1,
            ..request.clone()
        })
        .await
        .unwrap();
    assert_eq!(byte_limited.events.len(), 1);
    assert_eq!(byte_limited.next_offset, Some(offsets[0]));
    let head = live
        .read_by_handle(StreamHandleReadRequest {
            max_items: 0,
            max_bytes: 0,
            ..request.clone()
        })
        .await
        .unwrap();
    assert!(head.events.is_empty());
    assert_eq!(head.next_offset, None);
    assert_eq!(head.head_offset, Some(offsets[2]));
    for after in [
        offsets[0],
        offsets[2],
        StreamOffset::new(OplogIndex::from_u64(999), 0),
    ] {
        let head = live
            .read_by_handle(StreamHandleReadRequest {
                after: Some(after),
                max_items: 0,
                max_bytes: 0,
                ..request.clone()
            })
            .await
            .unwrap();
        assert_eq!(head.next_offset, Some(after));
        assert_eq!(head.head_offset, Some(offsets[2]));
        assert!(head.events.is_empty());
    }
    let wait = StreamHandleReadRequest {
        after: Some(offsets[2]),
        wait_millis: 5_000,
        ..request.clone()
    };
    let bus = live.stream_bus(handle.stream_id).await.unwrap();
    let mut attached_readers = Vec::new();
    for _ in 0..golem_common::base_model::durable_stream::MAX_LIVE_READERS_PER_STREAM {
        attached_readers.push(bus.subscribe().await.unwrap());
    }
    let left = live.read_by_handle(wait.clone());
    let right = live.read_by_handle(wait);
    tokio::pin!(left, right);
    assert!(futures::poll!(&mut left).is_pending());
    assert!(futures::poll!(&mut right).is_pending());
    assert!(matches!(
        bus.subscribe().await,
        Err(super::DurableLiveStreamBusError::ReaderLimit)
    ));
    let next = live
        .write_items(
            None,
            handle.stream_id,
            3,
            StreamItemsPayload::PackedU8(vec![41]),
        )
        .await
        .unwrap()
        .value[0];
    let (left, right) = tokio::join!(left, right);
    let left = left.unwrap();
    assert_eq!(left, right.unwrap());
    assert_eq!(left.events.len(), 1);
    assert_eq!(
        left.events[0].payload,
        CommittedProducerStreamEventPayload::PackedU8(41)
    );
    assert_eq!(left.next_offset, Some(next));
    assert_eq!(live.index.lock().await.attachments.len(), 1);
    let attached = live
        .read_attached_segment(&key, &handle, 102, None, None)
        .await
        .unwrap();
    assert_eq!(attached.len(), 4);
    let beyond = StreamOffset::new(
        OplogIndex::from_u64(next.producer_oplog_index().as_u64() + 100),
        0,
    );
    let result = live
        .read_by_handle(StreamHandleReadRequest {
            after: Some(beyond),
            ..request
        })
        .await
        .unwrap();
    assert!(result.events.is_empty());
    assert_eq!(result.next_offset, Some(beyond));
}

#[test]
#[test_r::timeout("30s")]
async fn future_cursor_live_reads_wait_for_deadline_or_closure() {
    use golem_common::model::durable_stream::StreamHandleReadRequest;
    let identity = identity();
    let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let request = StreamHandleReadRequest {
        handle: handle.clone(),
        after: Some(StreamOffset::new(OplogIndex::from_u64(999), 0)),
        max_items: 10,
        max_bytes: 100,
        wait_millis: 100,
    };
    let started = tokio::time::Instant::now();
    let timed_out = live.read_by_handle(request.clone()).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(timed_out.events.is_empty());
    assert_eq!(timed_out.next_offset, request.after);
    assert!(!timed_out.closed);

    let mut waiting = Box::pin(live.read_by_handle(StreamHandleReadRequest {
        wait_millis: 5_000,
        ..request.clone()
    }));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    live.write_items(
        None,
        handle.stream_id,
        0,
        StreamItemsPayload::PackedU8(vec![7]),
    )
    .await
    .unwrap();
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    live.end(None, handle.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap();
    let closed = waiting.await.unwrap();
    assert!(closed.closed);
    assert!(closed.events.is_empty());
    assert_eq!(closed.next_offset, request.after);
}

#[test]
#[test_r::timeout("30s")]
async fn normal_and_external_batches_publish_in_commit_order_after_caller_abort() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog, &identity, Some(1)).await;
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let bus = live.bus(handle.stream_id).unwrap();
    let mut reader = bus.subscribe().await.unwrap();
    let first = tokio::spawn({
        let live = live.clone();
        let id = handle.stream_id;
        async move {
            live.write_items(None, id, 0, StreamItemsPayload::PackedU8(vec![3, 7, 19]))
                .await
        }
    });
    while live.stream_head(&handle).await.unwrap().offset.is_none() {
        tokio::task::yield_now().await;
    }
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let second = live.append_external_input(
        None,
        &identity.invocation,
        handle.stream_id,
        Some(StreamItemsPayload::PackedU8(vec![31, 44])),
        true,
        None,
    );
    let read = async {
        let mut events = Vec::new();
        for _ in 0..6 {
            events.push(reader.recv().await.unwrap());
        }
        events
    };
    let (accepted, events) = tokio::join!(second, read);
    assert_eq!(
        events
            .iter()
            .map(|event| event.payload.payload.clone())
            .collect::<Vec<_>>(),
        vec![
            CommittedProducerStreamEventPayload::PackedU8(3),
            CommittedProducerStreamEventPayload::PackedU8(7),
            CommittedProducerStreamEventPayload::PackedU8(19),
            CommittedProducerStreamEventPayload::PackedU8(31),
            CommittedProducerStreamEventPayload::PackedU8(44),
            CommittedProducerStreamEventPayload::End(StreamEndResult::Ok),
        ]
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].offset < pair[1].offset)
    );
    assert_eq!(
        accepted.unwrap(),
        ExternalAppendOutcome::Accepted(events[5].offset)
    );
}

#[test]
#[test_r::timeout("30s")]
async fn failed_commit_callbacks_fence_cached_reads_and_recover_committed_items() {
    use golem_common::model::durable_stream::StreamHandleReadRequest;

    for fail_after_receipt in [false, true] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let fail = Arc::new(AtomicBool::new(false));
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let fail = fail.clone();
            move |receipt| {
                let oplog = oplog.clone();
                let fail = fail.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    assert!(
                        fail_after_receipt || !fail.load(Ordering::Acquire),
                        "injected failure before durability receipt"
                    );
                    if let Some(receipt) = receipt {
                        let _ = receipt.send(());
                    }
                    assert!(
                        !fail.load(Ordering::Acquire),
                        "injected failure after durability receipt"
                    );
                })
            }
        });
        let live = DurableStreamStore::load_with_commit(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let handle = live
            .register(None, root_registration(&identity))
            .await
            .unwrap()
            .value;
        let payload = StreamItemsPayload::PackedU8(vec![13, 79]);
        fail.store(true, Ordering::Release);
        assert!(
            live.write_items(None, handle.stream_id, 0, payload.clone())
                .await
                .is_err()
        );
        let request = StreamHandleReadRequest {
            handle: handle.clone(),
            after: None,
            max_items: 16,
            max_bytes: 4096,
            wait_millis: 0,
        };
        assert!(
            live.read_by_handle(request.clone()).await.is_err(),
            "cached reads must not conceal an uncertain commit outcome"
        );
        assert!(
            live.write_items(None, handle.stream_id, 0, payload.clone())
                .await
                .is_err()
        );

        let recovered = DurableStreamStore::load(
            oplog,
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let read = recovered.read_by_handle(request).await.unwrap();
        assert_eq!(
            read.events
                .iter()
                .map(|event| event.payload.clone())
                .collect::<Vec<_>>(),
            vec![
                CommittedProducerStreamEventPayload::PackedU8(13),
                CommittedProducerStreamEventPayload::PackedU8(79)
            ]
        );
        let retry = recovered
            .write_items(None, handle.stream_id, 0, payload)
            .await
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(
            retry.value,
            read.events
                .iter()
                .map(|event| event.offset)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn handle_read_hydrates_cancellation_committed_before_request_abort() {
    use golem_common::model::durable_stream::StreamHandleReadRequest;

    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let block_commit = Arc::new(AtomicBool::new(false));
    let committed = Arc::new(Notify::new());
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let block_commit = block_commit.clone();
        let committed = committed.clone();
        move |published| {
            let oplog = oplog.clone();
            let block_commit = block_commit.clone();
            let committed = committed.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if let Some(published) = published {
                    let _ = published.send(());
                }
                if block_commit.load(Ordering::SeqCst) {
                    committed.notify_waiters();
                    futures::future::pending().await
                }
            })
        }
    });
    let live = DurableStreamStore::load_with_commit(
        oplog,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let handle = live
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;

    block_commit.store(true, Ordering::SeqCst);
    let notification = committed.notified();
    let cancellation = tokio::spawn({
        let live = live.clone();
        async move {
            live.cancel_open(
                None,
                handle.stream_id,
                StreamCancelRole::OutputConsumer,
                StreamCancelReason::Cancelled,
                None,
            )
            .await
        }
    });
    notification.await;
    cancellation.abort();
    assert!(cancellation.await.unwrap_err().is_cancelled());

    let read = live
        .read_by_handle(StreamHandleReadRequest {
            handle,
            after: None,
            max_items: 0,
            max_bytes: 0,
            wait_millis: 0,
        })
        .await
        .unwrap();
    assert!(
        read.closed,
        "durably committed cancellation was not hydrated"
    );
    assert!(
        read.cancelled,
        "durably committed cancellation was not hydrated"
    );
}

#[test]
#[test_r::timeout("30s")]
async fn external_append_retry_after_commit_cancellation_is_duplicate() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let block_commit = Arc::new(AtomicBool::new(false));
    let committed = Arc::new(Notify::new());
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let block_commit = block_commit.clone();
        let committed = committed.clone();
        move |published| {
            let oplog = oplog.clone();
            let block_commit = block_commit.clone();
            let committed = committed.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if let Some(published) = published {
                    let _ = published.send(());
                }
                if block_commit.load(Ordering::SeqCst) {
                    committed.notify_waiters();
                    futures::future::pending().await
                }
            })
        }
    });
    let live = DurableStreamStore::load_with_commit(
        oplog,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let external = ExternalProducer {
        id: ExternalProducerId::Client("retrying-producer".into()),
        epoch: 1,
        sequence: 0,
    };

    block_commit.store(true, Ordering::SeqCst);
    let notification = committed.notified();
    let append = tokio::spawn({
        let live = live.clone();
        let session = identity.invocation.clone();
        let external = external.clone();
        async move {
            live.append_external_input(
                None,
                &session,
                handle.stream_id,
                Some(StreamItemsPayload::PackedU8(vec![7])),
                false,
                Some(external),
            )
            .await
        }
    });
    notification.await;
    append.abort();
    assert!(append.await.unwrap_err().is_cancelled());
    block_commit.store(false, Ordering::SeqCst);

    let retry = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![7])),
            false,
            Some(external),
        )
        .await
        .unwrap();
    assert!(
        matches!(retry, ExternalAppendOutcome::Duplicate { .. }),
        "durably committed producer sequence was appended again: {retry:?}"
    );
}

#[test]
#[test_r::timeout("30s")]
async fn external_append_survives_caller_abort_before_commit_receipt() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let block_receipt = Arc::new(AtomicBool::new(false));
    let blocked = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let block_receipt = block_receipt.clone();
        let blocked = blocked.clone();
        let release = release.clone();
        move |published| {
            let oplog = oplog.clone();
            let block_receipt = block_receipt.clone();
            let blocked = blocked.clone();
            let release = release.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if block_receipt.swap(false, Ordering::SeqCst) {
                    blocked.notify_one();
                    release.notified().await;
                }
                if let Some(published) = published {
                    let _ = published.send(());
                }
            })
        }
    });
    let live = DurableStreamStore::load_with_commit(
        oplog,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let external = ExternalProducer {
        id: ExternalProducerId::Client("retrying-producer".into()),
        epoch: 1,
        sequence: 0,
    };
    block_receipt.store(true, Ordering::SeqCst);
    let append = tokio::spawn({
        let live = live.clone();
        let session = identity.invocation.clone();
        let external = external.clone();
        async move {
            live.append_external_input(
                None,
                &session,
                handle.stream_id,
                Some(StreamItemsPayload::PackedU8(vec![7])),
                false,
                Some(external),
            )
            .await
        }
    });
    blocked.notified().await;
    append.abort();
    assert!(append.await.unwrap_err().is_cancelled());
    release.notify_one();
    let retry = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![7])),
            false,
            Some(external),
        )
        .await
        .unwrap();
    assert!(matches!(retry, ExternalAppendOutcome::Duplicate { .. }));
    assert!(live.ensure_healthy().is_ok());
}

// Provisional bug-finder test: external input records must retain their stream owner.
#[test]
async fn provisional_external_append_retains_entity_attribution() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let attribution = Some(OplogIndex::from_u64(71));
    let mut request = registration(
        &identity,
        StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKind::MethodInput,
            recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
        },
        StreamSourceKind::ExternalInlineInput,
    );
    request.entity_parent_start_index = attribution;
    let handle = live.register(None, request).await.unwrap().value;
    let before = oplog.current_oplog_index().await;

    live.append_external_input(
        None,
        &identity.invocation,
        handle.stream_id,
        Some(StreamItemsPayload::PackedU8(vec![7])),
        true,
        Some(ExternalProducer {
            id: ExternalProducerId::Client("attributed-producer".into()),
            epoch: 1,
            sequence: 0,
        }),
    )
    .await
    .unwrap();

    let after = oplog.current_oplog_index().await;
    for position in before.next().as_u64()..=after.as_u64() {
        assert_eq!(
            oplog
                .read(OplogIndex::from_u64(position))
                .await
                .entity_parent_start_index(),
            attribution
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn attached_input_distinguishes_foreign_close_from_its_own_end_after_reload() {
    for sent_item in [false, true] {
        for own_end in [false, true] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let live = producer(oplog.clone(), &identity, None).await;
            let handle = live
                .register(
                    None,
                    registration(
                        &identity,
                        StreamRegistrationCoordinate::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKind::MethodInput,
                            recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
                        },
                        StreamSourceKind::ExternalInlineInput,
                    ),
                )
                .await
                .unwrap()
                .value;
            if sent_item {
                live.write_attached_items_with_nested(
                    None,
                    &identity.invocation,
                    handle.stream_id,
                    0,
                    StreamItemsPayload::PackedU8(vec![13]),
                    Vec::new(),
                )
                .await
                .unwrap();
            }
            let end_sequence = u64::from(sent_item);
            live.append_external_input(
                None,
                &identity.invocation,
                handle.stream_id,
                None,
                true,
                own_end.then_some(ExternalProducer {
                    id: ExternalProducerId::Attached,
                    epoch: 0,
                    sequence: end_sequence,
                }),
            )
            .await
            .unwrap();
            drop(live);

            let recovered = producer(oplog.clone(), &identity, None).await;
            let tip = oplog.current_oplog_index().await;
            let error = recovered
                .write_attached_items_with_nested(
                    None,
                    &identity.invocation,
                    handle.stream_id,
                    end_sequence + u64::from(own_end),
                    StreamItemsPayload::PackedU8(vec![17]),
                    Vec::new(),
                )
                .await
                .unwrap_err();
            assert_eq!(
                error,
                if own_end {
                    StreamStoreError::FencedByTerminal(CommittedProducerStreamEventPayload::End(
                        StreamEndResult::Ok,
                    ))
                } else {
                    StreamStoreError::ClosedByOtherProducer
                }
            );
            if !own_end {
                assert_eq!(
                    recovered
                        .write_attached_items_with_nested(
                            None,
                            &identity.invocation,
                            handle.stream_id,
                            end_sequence + 2,
                            StreamItemsPayload::PackedU8(vec![23]),
                            Vec::new(),
                        )
                        .await
                        .unwrap_err(),
                    StreamStoreError::ClosedByOtherProducer,
                    "all frames already in flight must be discarded after a foreign close",
                );
            }
            if own_end {
                assert_eq!(
                    recovered
                        .write_attached_items_with_nested(
                            None,
                            &identity.invocation,
                            handle.stream_id,
                            end_sequence,
                            StreamItemsPayload::PackedU8(vec![19]),
                            Vec::new(),
                        )
                        .await
                        .unwrap_err(),
                    StreamStoreError::EventConflict,
                );
            }
            assert_eq!(oplog.current_oplog_index().await, tip);
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn attached_and_client_appends_interleave_and_reconstruct_multi_value_retry() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(98)],
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;

    live.write_attached_items_with_nested(
        None,
        &identity.invocation,
        handle.stream_id,
        0,
        StreamItemsPayload::PackedU8(vec![10]),
        Vec::new(),
    )
    .await
    .unwrap();
    let client = ExternalProducer {
        id: ExternalProducerId::Client("multi".into()),
        epoch: 0,
        sequence: 0,
    };
    let accepted = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::Values(vec![vec![20], vec![30]])),
            false,
            Some(client.clone()),
        )
        .await
        .unwrap();
    live.write_attached_items_with_nested(
        None,
        &identity.invocation,
        handle.stream_id,
        1,
        StreamItemsPayload::PackedU8(vec![40]),
        Vec::new(),
    )
    .await
    .unwrap();
    drop(live);

    let recovered = producer(oplog.clone(), &identity, None).await;
    let ExternalAppendOutcome::Accepted(original_offset) = accepted else {
        panic!("client append was not accepted")
    };
    assert_eq!(
        recovered
            .append_external_input(
                None,
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayload::Values(vec![vec![20], vec![30]])),
                false,
                Some(client),
            )
            .await
            .unwrap(),
        ExternalAppendOutcome::Duplicate {
            offset: original_offset,
            highest_sequence: Some(0),
        }
    );
    assert_eq!(
        recovered
            .attached_input_high_water(&identity.invocation, handle.stream_id)
            .await
            .unwrap()
            .unwrap()
            .highest_contiguous_sequence,
        1
    );
    let retry = recovered
        .write_attached_items_with_nested(
            None,
            &identity.invocation,
            handle.stream_id,
            1,
            StreamItemsPayload::PackedU8(vec![40]),
            Vec::new(),
        )
        .await
        .unwrap();
    assert!(retry.replayed);
    assert!(matches!(
        recovered
            .write_attached_items_with_nested(
                None,
                &identity.invocation,
                handle.stream_id,
                1,
                StreamItemsPayload::PackedU8(vec![41]),
                Vec::new(),
            )
            .await,
        Err(StreamStoreError::EventConflict)
    ));
    recovered
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            None,
            true,
            Some(ExternalProducer {
                id: ExternalProducerId::Attached,
                epoch: 0,
                sequence: 2,
            }),
        )
        .await
        .unwrap();
    let events = recovered.read_segment(&handle, None, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.producer_sequence)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert!(matches!(
        events[0].payload,
        CommittedProducerStreamEventPayload::PackedU8(10)
    ));
    assert_eq!(
        events[1].payload,
        CommittedProducerStreamEventPayload::Value(vec![20])
    );
    assert_eq!(
        events[2].payload,
        CommittedProducerStreamEventPayload::Value(vec![30])
    );
    assert!(matches!(
        events[3].payload,
        CommittedProducerStreamEventPayload::PackedU8(40)
    ));
    assert!(events[4].is_terminal());

    let item_records = oplog
        .read_exact(
            OplogIndex::INITIAL,
            oplog.current_oplog_index().await.as_u64(),
        )
        .await
        .into_iter()
        .filter(|(_, entry)| matches!(entry, OplogEntry::StreamItems { .. }))
        .count();
    assert_eq!(
        item_records, 4,
        "each value append must have its own Items record"
    );
}

#[test]
#[timeout("30s")]
async fn attached_packed_and_nested_inputs_keep_transport_sequences_during_http_writes() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(97)],
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let (attached, http) = tokio::join!(
        live.write_attached_items_with_nested(
            None,
            &identity.invocation,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![10, 11]),
            Vec::new(),
        ),
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::Values(vec![
                vec![20],
                vec![30],
                vec![31]
            ])),
            false,
            None,
        ),
    );
    assert_eq!(attached.unwrap().value.len(), 2);
    assert!(matches!(http.unwrap(), ExternalAppendOutcome::Accepted(_)));
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: handle.stream_id,
            parent_producer_sequence: 2,
            recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
        },
        StreamSourceKind::Nested,
    );
    let payload = StreamItemsPayload::Values(vec![vec![40]]);
    let fresh = live
        .write_attached_items_with_nested(
            None,
            &identity.invocation,
            handle.stream_id,
            2,
            payload.clone(),
            vec![nested.clone()],
        )
        .await
        .unwrap();
    assert!(!fresh.replayed);
    assert_eq!(
        live.attached_global_sequence(&identity.invocation, handle.stream_id, 2)
            .await
            .unwrap(),
        5
    );
    let child = live
        .nested_handles(handle.stream_id, 5)
        .await
        .unwrap()
        .pop()
        .unwrap();
    live.append_external_input(
        None,
        &identity.invocation,
        child.stream_id,
        None,
        true,
        Some(ExternalProducer {
            id: ExternalProducerId::Attached,
            epoch: 0,
            sequence: 0,
        }),
    )
    .await
    .unwrap();
    drop(live);
    let recovered = producer(oplog, &identity, None).await;
    let retry = recovered
        .write_attached_items_with_nested(
            None,
            &identity.invocation,
            handle.stream_id,
            2,
            payload,
            vec![nested],
        )
        .await
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.value, fresh.value);
    assert_eq!(
        recovered.nested_handles(handle.stream_id, 5).await.unwrap(),
        vec![child]
    );
    let high_water = recovered
        .attached_input_high_water(&identity.invocation, handle.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(high_water.highest_contiguous_sequence, 2);
    assert!(!high_water.terminal);
    assert!(matches!(
        recovered
            .append_external_input(
                None,
                &identity.invocation,
                handle.stream_id,
                None,
                true,
                Some(ExternalProducer {
                    id: ExternalProducerId::Attached,
                    epoch: 0,
                    sequence: 2
                }),
            )
            .await,
        Err(StreamStoreError::EventConflict)
    ));
    recovered
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            None,
            true,
            None,
        )
        .await
        .unwrap();
    let high_water = recovered
        .attached_input_high_water(&identity.invocation, handle.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(high_water.highest_contiguous_sequence, 3);
    assert!(high_water.terminal);
    let events = recovered.read_segment(&handle, None, None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.producer_sequence)
            .collect::<Vec<_>>(),
        (0..7).collect::<Vec<_>>()
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].offset < pair[1].offset)
    );
}

#[test]
async fn external_append_producer_retries_epochs_close_and_recovery() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(99)],
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let p0 = ExternalProducer {
        id: ExternalProducerId::Client("p1".into()),
        epoch: 1,
        sequence: 0,
    };
    let accepted = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![1, 2])),
            false,
            Some(p0.clone()),
        )
        .await
        .unwrap();
    let ExternalAppendOutcome::Accepted(original) = accepted else {
        panic!()
    };
    let (a, b) = tokio::join!(
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![1, 2])),
            false,
            Some(p0.clone())
        ),
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![1, 2])),
            false,
            Some(p0)
        ),
    );
    let duplicate = ExternalAppendOutcome::Duplicate {
        offset: original,
        highest_sequence: Some(0),
    };
    assert_eq!(a.unwrap(), duplicate);
    assert_eq!(b.unwrap(), duplicate);
    let newer = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![3])),
            false,
            Some(ExternalProducer {
                id: ExternalProducerId::Client("p1".into()),
                epoch: 1,
                sequence: 1,
            }),
        )
        .await
        .unwrap();
    assert!(matches!(newer, ExternalAppendOutcome::Accepted(_)));
    assert_eq!(
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![1, 2])),
            false,
            Some(ExternalProducer {
                id: ExternalProducerId::Client("p1".into()),
                epoch: 1,
                sequence: 0,
            }),
        )
        .await
        .unwrap(),
        ExternalAppendOutcome::Duplicate {
            offset: original,
            highest_sequence: Some(1),
        }
    );
    assert_eq!(
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![3])),
            false,
            Some(ExternalProducer {
                id: ExternalProducerId::Client("p1".into()),
                epoch: 1,
                sequence: 3
            })
        )
        .await
        .unwrap(),
        ExternalAppendOutcome::SeqGap {
            expected: 2,
            received: 3
        }
    );
    assert!(matches!(
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![3])),
            false,
            Some(ExternalProducer {
                id: ExternalProducerId::Client("p1".into()),
                epoch: 2,
                sequence: 0
            })
        )
        .await
        .unwrap(),
        ExternalAppendOutcome::Accepted(_)
    ));
    assert_eq!(
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![4])),
            false,
            Some(ExternalProducer {
                id: ExternalProducerId::Client("p1".into()),
                epoch: 1,
                sequence: 1
            })
        )
        .await
        .unwrap(),
        ExternalAppendOutcome::EpochFenced(2)
    );
    let normal = live
        .write_items(
            None,
            handle.stream_id,
            4,
            StreamItemsPayload::PackedU8(vec![8]),
        )
        .await
        .unwrap()
        .value[0];
    let closed = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![4])),
            true,
            Some(ExternalProducer {
                id: ExternalProducerId::Client("p2".into()),
                epoch: 0,
                sequence: 0,
            }),
        )
        .await
        .unwrap();
    let ExternalAppendOutcome::Accepted(closed) = closed else {
        panic!()
    };
    assert!(original < normal && normal < closed);
    assert_eq!(
        live.append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            None,
            true,
            None
        )
        .await
        .unwrap(),
        ExternalAppendOutcome::Duplicate {
            offset: closed,
            highest_sequence: None,
        }
    );
    drop(live);
    let recovered = producer(oplog, &identity, None).await;
    assert_eq!(
        recovered
            .append_external_input(
                None,
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayload::PackedU8(vec![9])),
                false,
                None
            )
            .await
            .unwrap(),
        ExternalAppendOutcome::Closed
    );
}

#[test]
async fn close_from_a_different_producer_is_not_reported_as_duplicate() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    let handle = live
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let producer_tuple = |id: &str, sequence| ExternalProducer {
        id: ExternalProducerId::Client(id.into()),
        epoch: 0,
        sequence,
    };
    live.append_external_input(
        None,
        &identity.invocation,
        handle.stream_id,
        Some(StreamItemsPayload::PackedU8(vec![1])),
        false,
        Some(producer_tuple("first", 0)),
    )
    .await
    .unwrap();
    let accepted = live
        .append_external_input(
            None,
            &identity.invocation,
            handle.stream_id,
            None,
            true,
            Some(producer_tuple("first", 1)),
        )
        .await
        .unwrap();
    let ExternalAppendOutcome::Accepted(offset) = accepted else {
        panic!("first producer close must be accepted");
    };
    let before = oplog.current_oplog_index().await;
    let recovered = producer(oplog.clone(), &identity, None).await;

    for current in [&live, &recovered] {
        assert_eq!(
            current
                .append_external_input(
                    None,
                    &identity.invocation,
                    handle.stream_id,
                    None,
                    true,
                    Some(producer_tuple("first", 1)),
                )
                .await
                .unwrap(),
            ExternalAppendOutcome::Duplicate {
                offset,
                highest_sequence: Some(1),
            }
        );
        for request in [
            producer_tuple("different", 0),
            producer_tuple("first", 2),
            ExternalProducer {
                epoch: 1,
                ..producer_tuple("first", 1)
            },
        ] {
            assert_eq!(
                current
                    .append_external_input(
                        None,
                        &identity.invocation,
                        handle.stream_id,
                        None,
                        true,
                        Some(request),
                    )
                    .await
                    .unwrap(),
                ExternalAppendOutcome::Closed
            );
        }
        assert_eq!(
            current
                .append_external_input(
                    None,
                    &identity.invocation,
                    handle.stream_id,
                    Some(StreamItemsPayload::PackedU8(vec![1])),
                    false,
                    Some(producer_tuple("first", 0)),
                )
                .await
                .unwrap(),
            ExternalAppendOutcome::Closed
        );
        assert_eq!(oplog.current_oplog_index().await, before);
    }
}

#[test]
async fn producer_frames_after_earlier_input_consumer_cancel_are_fenced() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let handle = producer
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKind::ExternalInlineInput,
            ),
        )
        .await
        .unwrap()
        .value;
    producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1]]),
        )
        .await
        .unwrap();
    producer
        .cancel_open(
            None,
            handle.stream_id,
            StreamCancelRole::InputConsumer,
            StreamCancelReason::GuestDrop,
            None,
        )
        .await
        .unwrap();
    let oplog_length = oplog.committed_length();

    assert!(matches!(
        producer
            .write_items(
                None,
                handle.stream_id,
                1,
                StreamItemsPayload::Values(vec![vec![2]]),
            )
            .await,
        Err(StreamStoreError::FencedByTerminal(
            CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputConsumer,
                reason: StreamCancelReason::GuestDrop,
                details: None,
            }
        ))
    ));
    assert_eq!(oplog.committed_length(), oplog_length);

    assert!(matches!(
        producer
            .end(None, handle.stream_id, 64, StreamEndResult::Ok)
            .await,
        Err(StreamStoreError::FencedByTerminal(
            CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputConsumer,
                reason: StreamCancelReason::GuestDrop,
                details: None,
            }
        ))
    ));
    assert_eq!(oplog.committed_length(), oplog_length);
}

#[test]
async fn prepared_input_registration_batch_recovers_without_duplicate_registration() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let commit_reached = Arc::new(Barrier::new(2));
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let commit_reached = commit_reached.clone();
        move |committed| {
            let oplog = oplog.clone();
            let commit_reached = commit_reached.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if let Some(committed) = committed {
                    let _ = committed.send(());
                }
                commit_reached.wait().await;
                std::future::pending::<()>().await;
            })
        }
    });
    let live_producer = DurableStreamStore::load_with_commit(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let registration = registration(
        &identity,
        StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKind::MethodInput,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKind::ExternalInlineInput,
    );
    let session_key = identity.invocation.clone();
    let callee_fingerprint = identity.fingerprint;
    let attachment_id = AttachmentId::primary(
        session_key.callee_environment_id,
        &session_key.callee,
        &session_key.idempotency_key,
    )
    .unwrap();
    let attempt_id = AttemptId(Uuid::new_v4());
    let pending = OplogEntry::pending_agent_invocation(
        IdempotencyKey::new("durable-session".to_string()),
        OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
        TraceId::generate(),
        Vec::new(),
        Vec::new(),
    );
    let (committed, committed_rx) = oneshot::channel();
    let preparation = tokio::spawn({
        let live_producer = live_producer.clone();
        let registration = registration.clone();
        async move {
            live_producer
                .prepare_session(
                    None,
                    vec![(17, registration)],
                    pending,
                    committed,
                    move |bindings| {
                        let handles = bindings
                            .iter()
                            .map(|(_, handle)| handle.clone())
                            .collect::<Vec<_>>();
                        StreamSessionPreparedRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            attempt: StartAttemptDescriptor {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: session_key.clone(),
                                attachment_id,
                                expected_callee_fingerprint: callee_fingerprint,
                                attempt_id,
                                invocation: PersistedStreamInvocationDescriptor {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    session_key,
                                    target_component_revision: ComponentRevision::INITIAL,
                                    method_name: "consume".to_string(),
                                    invocation_value: vec![1],
                                    stream_handles: handles,
                                    execution_config: vec![2],
                                    effective_identity: vec![3],
                                },
                                effective_identity: vec![3],
                                live_join_buffer_events: 8,
                            },
                            stream_mappings: bindings
                                .into_iter()
                                .map(|(transport_stream_id, handle)| StreamSessionMappingRecord {
                                    transport_stream_id,
                                    handle,
                                    role: SessionStreamRole::Input,
                                })
                                .collect(),
                        }
                    },
                )
                .await
        }
    });
    committed_rx.await.unwrap();
    commit_reached.wait().await;
    assert_eq!(oplog.committed_length(), 4);
    assert_eq!(oplog.commit_count(), 1);
    preparation.abort();
    assert!(preparation.await.unwrap_err().is_cancelled());
    drop(live_producer);

    let entries = oplog.entries();
    assert!(matches!(entries[0], OplogEntry::StreamRegistered { .. }));
    let OplogEntry::StreamSession {
        record: OplogPayload::Inline(prepared),
        ..
    } = &entries[1]
    else {
        panic!("acceptance batch must contain an inline Prepared record");
    };
    let StreamSessionRecord::Prepared(prepared) = prepared.as_ref() else {
        panic!("acceptance batch must contain a Prepared record");
    };
    assert!(matches!(
        entries[2],
        OplogEntry::PendingAgentInvocation { .. }
    ));
    let OplogEntry::StreamSession {
        record: OplogPayload::Inline(attached),
        ..
    } = &entries[3]
    else {
        panic!("acceptance batch must end with an inline Attached record");
    };
    let StreamSessionRecord::Attached(attached) = attached.as_ref() else {
        panic!("acceptance batch must end with an Attached record");
    };
    assert_eq!(attached.pending_invocation_oplog_index.as_u64(), 3);
    assert_eq!(prepared.stream_mappings.len(), 1);

    let restarted = producer(oplog.clone(), &identity, None).await;
    let recovered = restarted
        .validate_registration(&registration)
        .await
        .unwrap();
    assert_eq!(recovered, prepared.stream_mappings[0].handle);
    assert_eq!(oplog.committed_length(), 4);
}

#[test]
async fn protocol_terminalization_closes_an_open_stream_once() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![42]]),
        )
        .await
        .unwrap();

    producer
        .end_open(
            None,
            handle.stream_id,
            StreamEndResult::ErrorContext(b"invocation failed".to_vec()),
        )
        .await
        .unwrap();
    let committed_length = oplog.committed_length();
    producer
        .end_open(
            None,
            handle.stream_id,
            StreamEndResult::ErrorContext(b"ignored duplicate".to_vec()),
        )
        .await
        .unwrap();

    assert_eq!(oplog.committed_length(), committed_length);
    let mut reader = producer.catch_up(handle, None).await.unwrap();
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::Value(_)
    ));
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(details))
            if details == b"invocation failed"
    ));
    assert!(reader.next().await.unwrap().is_none());
}

#[test]
async fn empty_invocation_result_replays_exactly_and_rejects_conflicts() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;

    producer
        .register_result_streams(None, identity.invocation.clone(), vec![1], Vec::new(), None)
        .await
        .unwrap();
    let committed = oplog.committed_length();

    producer
        .register_result_streams(None, identity.invocation.clone(), vec![1], Vec::new(), None)
        .await
        .unwrap();
    assert_eq!(oplog.committed_length(), committed);

    assert_eq!(
        producer
            .register_result_streams(None, identity.invocation.clone(), vec![2], Vec::new(), None)
            .await
            .unwrap_err(),
        StreamStoreError::RegistrationDivergence
    );
    assert_eq!(oplog.committed_length(), committed);
}

#[test]
async fn result_plan_preserves_mixed_output_order_and_replays() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let existing = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let mut request = root_registration(&identity);
    let StreamRegistrationCoordinate::Root {
        recursive_value_path,
        ..
    } = &mut request.coordinate
    else {
        unreachable!();
    };
    recursive_value_path.push(StreamValuePathStep::RecordField(1));
    let outputs = || {
        vec![
            ProducerOutputRegistration {
                transport_stream_id: 31,
                source: ProducerOutputSource::New(request.clone()),
                cancellation_epoch: None,
            },
            ProducerOutputRegistration {
                transport_stream_id: 12,
                source: ProducerOutputSource::Existing(existing.clone()),
                cancellation_epoch: None,
            },
        ]
    };
    let registered = producer
        .register_result_streams(
            None,
            identity.invocation.clone(),
            vec![4, 5],
            outputs(),
            None,
        )
        .await
        .unwrap();
    let owned = registered.handles;
    let record = registered.session_record;
    assert_eq!(owned.len(), 1);
    let StreamSessionRecord::InvocationResult(result) = &record else {
        unreachable!();
    };
    assert_eq!(result.session_key, identity.invocation);
    assert_eq!(result.result, vec![4, 5]);
    assert_eq!(
        result.output_streams,
        vec![owned[0].clone(), existing.clone()]
    );
    assert_eq!(
        result
            .stream_mappings
            .iter()
            .map(|mapping| mapping.transport_stream_id)
            .collect::<Vec<_>>(),
        vec![31, 12]
    );
    let committed = oplog.committed_length();
    let replay = producer
        .register_result_streams(
            None,
            identity.invocation.clone(),
            vec![4, 5],
            outputs(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        replay,
        ResultStreamRegistration {
            handles: owned,
            session_record: record,
        }
    );
    assert_eq!(oplog.committed_length(), committed);
}

#[test]
async fn result_registration_cancels_outputs_before_publishing_the_result() {
    for new_output in [false, true] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let mut request = root_registration(&identity);
        request.coordinate = StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKind::MethodResult,
            recursive_value_path: vec![StreamValuePathStep::RecordField(0)],
        };
        let existing = producer
            .register(None, request.clone())
            .await
            .unwrap()
            .value;
        producer
            .end(None, existing.stream_id, 0, StreamEndResult::Ok)
            .await
            .unwrap();
        request.coordinate = StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKind::MethodResult,
            recursive_value_path: vec![StreamValuePathStep::RecordField(1)],
        };
        let open = if new_output {
            None
        } else {
            Some(
                producer
                    .register(None, request.clone())
                    .await
                    .unwrap()
                    .value,
            )
        };
        let outputs = || {
            vec![
                ProducerOutputRegistration {
                    transport_stream_id: 13,
                    source: match &open {
                        Some(handle) => ProducerOutputSource::Existing(handle.clone()),
                        None => ProducerOutputSource::New(request.clone()),
                    },
                    cancellation_epoch: Some(7),
                },
                ProducerOutputRegistration {
                    transport_stream_id: 29,
                    source: ProducerOutputSource::Existing(existing.clone()),
                    cancellation_epoch: Some(7),
                },
            ]
        };
        let registered = producer
            .register_result_streams(None, identity.invocation.clone(), vec![3], outputs(), None)
            .await
            .unwrap();
        let StreamSessionRecord::InvocationResult(result) = &registered.session_record else {
            unreachable!()
        };
        let result_index = oplog.current_oplog_index().await;
        let recovered = DurableStreamStore::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        for (position, handle) in result.output_streams.iter().enumerate() {
            let mut reader = recovered.catch_up(handle.clone(), None).await.unwrap();
            let event = reader.next().await.unwrap().unwrap();
            assert!(event.offset.producer_oplog_index() < result_index);
            assert_eq!(
                event.payload,
                if position == 0 {
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::OutputConsumer,
                        reason: StreamCancelReason::Cancelled,
                        details: None,
                    }
                } else {
                    CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
                }
            );
            assert!(reader.next().await.unwrap().is_none());
        }
        assert_eq!(
            recovered
                .register_result_streams(
                    None,
                    identity.invocation.clone(),
                    vec![3],
                    outputs(),
                    None
                )
                .await
                .unwrap(),
            registered
        );
        assert_eq!(oplog.current_oplog_index().await, result_index);
    }
}

#[test]
async fn result_plan_rejects_duplicate_new_coordinates_before_committing() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let request = root_registration(&identity);
    let outputs = vec![
        ProducerOutputRegistration {
            transport_stream_id: 1,
            source: ProducerOutputSource::New(request.clone()),
            cancellation_epoch: None,
        },
        ProducerOutputRegistration {
            transport_stream_id: 2,
            source: ProducerOutputSource::New(request),
            cancellation_epoch: None,
        },
    ];

    assert_eq!(
        producer
            .register_result_streams(None, identity.invocation.clone(), vec![1], outputs, None)
            .await,
        Err(StreamStoreError::RegistrationDivergence)
    );
    assert_eq!(oplog.committed_length(), 0);
}

#[test]
async fn nested_registration_and_enclosing_item_share_one_ordered_batch() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let parent = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: parent.value.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKind::Nested,
    );
    let written = producer
        .write_items_with_nested(
            None,
            parent.value.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1, 2, 3]]),
            vec![nested.clone()],
        )
        .await
        .unwrap();
    assert_eq!(
        written.value[0].producer_oplog_index(),
        OplogIndex::from_u64(3)
    );
    let nested_replay = producer.register(None, nested).await.unwrap();
    assert!(nested_replay.replayed);
    assert_eq!(oplog.committed_length(), 3);
    assert!(
        producer
            .write_items_with_nested(
                None,
                parent.value.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1, 2, 3]]),
                vec![registration(
                    &identity,
                    StreamRegistrationCoordinate::Nested {
                        parent_stream_id: parent.value.stream_id,
                        parent_producer_sequence: 0,
                        recursive_value_path: Vec::new(),
                    },
                    StreamSourceKind::Nested,
                )],
            )
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(oplog.committed_length(), 3);
    let entries = oplog.read_exact(OplogIndex::INITIAL, 3).await;
    assert!(matches!(
        entries.get(&OplogIndex::from_u64(2)),
        Some(OplogEntry::StreamRegistered { .. })
    ));
    assert!(matches!(
        entries.get(&OplogIndex::from_u64(3)),
        Some(OplogEntry::StreamItems { .. })
    ));
}

#[test]
async fn new_nested_registration_cannot_commit_without_its_enclosing_item() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let parent = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: parent.value.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKind::Nested,
    );

    assert_eq!(
        producer.register(None, nested).await,
        Err(StreamStoreError::RegistrationDivergence)
    );
    assert_eq!(
        oplog.committed_length(),
        1,
        "same-producer nested registration must only commit in the enclosing item's batch"
    );
}

#[test]
async fn catch_up_joins_live_without_a_gap_or_duplicate() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let mut reader = producer
        .catch_up(registered.value.clone(), None)
        .await
        .unwrap();
    producer
        .write_items(
            None,
            registered.value.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![9]]),
        )
        .await
        .unwrap();
    producer
        .end(None, registered.value.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap();
    assert_eq!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::Value(vec![9])
    );
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));
    assert!(reader.next().await.unwrap().is_none());
}

#[test]
async fn replay_publication_is_deduplicated_at_the_live_reader() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let mut reader = producer
        .catch_up(registered.value.clone(), None)
        .await
        .unwrap();
    producer
        .write_items(
            None,
            registered.value.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1]]),
        )
        .await
        .unwrap();
    assert_eq!(reader.next().await.unwrap().unwrap().producer_sequence, 0);

    assert!(
        producer
            .write_items(
                None,
                registered.value.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1]]),
            )
            .await
            .unwrap()
            .replayed
    );
    producer
        .end(None, registered.value.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap();
    let terminal = reader.next().await.unwrap().unwrap();
    assert_eq!(terminal.producer_sequence, 1);
    assert!(matches!(
        terminal.payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));
}

#[test]
async fn malformed_history_is_rejected_while_rebuilding_the_index() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let stream_id = registered.value.stream_id;
    let producer_fingerprint = identity.fingerprint;
    oplog
        .add_durable_stream_batch(Box::new(move |item_index| {
            vec![DurableStreamOplogRecord::Items(
                None,
                StreamItemsRecord {
                    format_version: 1,
                    stream_id,
                    producer_fingerprint,
                    first_sequence: 1,
                    nested_stream_ids: Vec::new(),
                    newly_registered_stream_ids: Vec::new(),
                    payload: StreamItemsPayload::Values(vec![vec![1]]),
                    offsets: vec![StreamOffset::new(item_index, 0)],
                },
            )]
        }))
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await;
    drop(producer);

    assert!(matches!(
        DurableStreamStore::load(
            oplog,
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await,
        Err(super::StreamStoreError::SequenceGap {
            expected: 0,
            actual: 1,
        })
    ));
}

#[test]
async fn rejected_nested_item_batch_does_not_partially_mutate_the_stream_index() {
    let identity = identity();
    let mut index = ProducerStreamIndex::default();
    let root_index = OplogIndex::INITIAL;
    let root = registration_record(
        root_index,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        root_registration(&identity),
    );
    let parent_stream_id = root.handle.stream_id;
    index
        .apply_registration(
            root_index,
            None,
            root,
            identity.environment_id,
            &identity.agent_id,
            identity.fingerprint,
        )
        .unwrap();

    let nested_index = root_index.next();
    let nested = registration_record(
        nested_index,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        registration(
            &identity,
            StreamRegistrationCoordinate::Nested {
                parent_stream_id,
                parent_producer_sequence: 1,
                recursive_value_path: vec![StreamValuePathStep::OptionSome],
            },
            StreamSourceKind::Nested,
        ),
    );
    let nested_stream_id = nested.handle.stream_id;
    let item_index = nested_index.next();
    let error = index
        .apply_item_batch(
            item_index,
            None,
            vec![(nested_index, None, nested)],
            StreamItemsRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: parent_stream_id,
                producer_fingerprint: identity.fingerprint,
                first_sequence: 1,
                nested_stream_ids: vec![nested_stream_id],
                newly_registered_stream_ids: vec![nested_stream_id],
                payload: StreamItemsPayload::Values(vec![vec![1]]),
                offsets: vec![StreamOffset::new(item_index, 0)],
            },
            identity.environment_id,
            &identity.agent_id,
            identity.fingerprint,
        )
        .unwrap_err();

    assert_eq!(
        error,
        StreamStoreError::SequenceGap {
            expected: 0,
            actual: 1,
        }
    );
    assert!(!index.registrations.contains_key(&nested_stream_id));
    assert!(!index.streams.contains_key(&nested_stream_id));
    assert_eq!(index.registrations.len(), 1);
}

#[test]
async fn history_rebuild_rejects_duplicate_nested_stream_ownership() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let parent = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let parent_stream_id = parent.value.stream_id;
    let environment_id = identity.environment_id;
    let agent_id = identity.agent_id.clone();
    let producer_fingerprint = identity.fingerprint;
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: vec![StreamValuePathStep::OptionSome],
        },
        StreamSourceKind::Nested,
    );
    oplog
        .add_durable_stream_batch(Box::new(move |registration_index| {
            let nested_record = registration_record(
                registration_index,
                environment_id,
                agent_id,
                producer_fingerprint,
                nested,
            );
            let nested_stream_id = nested_record.handle.stream_id;
            let item_index = OplogIndex::from_u64(registration_index.as_u64() + 1);
            vec![
                DurableStreamOplogRecord::Registered(None, nested_record),
                DurableStreamOplogRecord::Items(
                    None,
                    StreamItemsRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id: parent_stream_id,
                        producer_fingerprint,
                        first_sequence: 0,
                        nested_stream_ids: vec![nested_stream_id, nested_stream_id],
                        newly_registered_stream_ids: vec![nested_stream_id],
                        payload: StreamItemsPayload::Values(vec![vec![1]]),
                        offsets: vec![StreamOffset::new(item_index, 0)],
                    },
                ),
            ]
        }))
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await;
    drop(producer);

    assert!(
        DurableStreamStore::load(
            oplog,
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .is_err(),
        "one affine nested stream cannot be owned twice by the same enclosing value"
    );
}

#[test]
async fn history_rebuild_rejects_nested_registration_without_enclosing_item() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let parent = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let environment_id = identity.environment_id;
    let agent_id = identity.agent_id.clone();
    let producer_fingerprint = identity.fingerprint;
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: parent.value.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: vec![StreamValuePathStep::OptionSome],
        },
        StreamSourceKind::Nested,
    );
    oplog
        .add_durable_stream_batch(Box::new(move |registration_index| {
            vec![DurableStreamOplogRecord::Registered(
                None,
                registration_record(
                    registration_index,
                    environment_id,
                    agent_id,
                    producer_fingerprint,
                    nested,
                ),
            )]
        }))
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await;
    drop(producer);

    assert!(matches!(
        DurableStreamStore::load(
            oplog,
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await,
        Err(StreamStoreError::CorruptHistory(_))
    ));
}

#[test]
async fn encoded_size_rejection_has_no_durable_effect_and_sequence_can_retry() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let stream_id = registered.value.stream_id;

    assert_eq!(
        producer
            .write_items(
                None,
                stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![0; MAX_DURABLE_STREAM_ITEM_SIZE + 1]]),
            )
            .await,
        Err(StreamStoreError::ItemTooLarge)
    );
    assert_eq!(oplog.committed_length(), 1);
    assert_eq!(
        producer
            .write_items(
                None,
                stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![0; MAX_PACKED_U8_STREAM_ITEM_SIZE + 1]),
            )
            .await,
        Err(StreamStoreError::InvalidPackedU8Batch)
    );
    assert_eq!(oplog.committed_length(), 1);

    let written = producer
        .write_items(
            None,
            stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1]]),
        )
        .await
        .unwrap();
    assert_eq!(
        written.value[0].producer_oplog_index(),
        OplogIndex::from_u64(2)
    );
}

#[test]
async fn root_registration_rejects_coordinate_beyond_traversal_depth_limit() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let request = registration(
        &identity,
        StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKind::MethodResult,
            recursive_value_path: (0..=MAX_STREAM_VALUE_TRAVERSAL_DEPTH)
                .map(|_| StreamValuePathStep::OptionSome)
                .collect(),
        },
        StreamSourceKind::InvocationOutput,
    );

    assert_eq!(
        producer.register(None, request).await,
        Err(StreamStoreError::TraversalDepthLimit)
    );
    assert_eq!(
        oplog.committed_length(),
        0,
        "an invalid initial descriptor must have no durable effect"
    );
}

#[test]
async fn rejects_out_of_range_join_capacity_before_registration() {
    let identity = identity();
    for invalid_capacity in [0, MAX_LIVE_JOIN_BUFFER_SIZE + 1] {
        let oplog = Arc::new(TestOplog::default());
        assert!(matches!(
            DurableStreamStore::load(
                oplog.clone(),
                identity.environment_id,
                identity.agent_id.clone(),
                identity.fingerprint,
                Some(invalid_capacity),
            )
            .await,
            Err(StreamStoreError::LiveBus(
                DurableLiveStreamBusError::InvalidCapacity
            ))
        ));
        assert_eq!(oplog.committed_length(), 0);
    }
}

#[test]
async fn stream_limit_is_scoped_to_one_session_not_the_producer_agent_lifetime() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;

    for session in 0..=MAX_DURABLE_STREAMS_PER_SESSION {
        let mut invocation = identity.invocation.clone();
        invocation.idempotency_key = IdempotencyKey::new(format!("session-{session}"));
        let request = ProducerRegistrationRequest {
            coordinate: StreamRegistrationCoordinate::Root {
                invocation_id: invocation.clone(),
                root_kind: StreamRootKind::MethodResult,
                recursive_value_path: Vec::new(),
            },
            source_invocation: invocation,
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
            source_kind: StreamSourceKind::InvocationOutput,
            session_mapping: None,
            entity_parent_start_index: None,
        };

        producer.register(None, request).await.expect(
            "one stream in each independent session must remain below the per-session limit",
        );
    }
}

#[test]
async fn foreign_mappings_are_deduplicated_and_count_toward_the_session_limit() {
    let identity = identity();
    let mapping = |position: usize| StreamSessionMappingRecord {
        transport_stream_id: position as u64,
        handle: golem_common::base_model::durable_stream::DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: StreamId(Uuid::from_u128(10_000 + position as u128)),
            producer_environment_id: EnvironmentId(Uuid::from_u128(41)),
            producer: AgentId {
                component_id: ComponentId(Uuid::from_u128(42)),
                agent_id: "foreign-producer".to_string(),
            },
            expected_producer_fingerprint: AgentFingerprint(Uuid::from_u128(43)),
            source_invocation: identity.invocation.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([9; 32]),
        },
        role: SessionStreamRole::Input,
    };
    let record = |mapping| {
        StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: identity.invocation.clone(),
            mapping,
        })
    };
    let mut index = ProducerStreamIndex::default();
    for position in 0..MAX_DURABLE_STREAMS_PER_SESSION {
        index
            .apply_session_references(None, &record(mapping(position)))
            .unwrap();
    }
    assert_eq!(
        index.session_stream_counts[&identity.invocation],
        MAX_DURABLE_STREAMS_PER_SESSION
    );

    index
        .apply_session_references(None, &record(mapping(0)))
        .unwrap();
    assert_eq!(
        index.session_stream_counts[&identity.invocation],
        MAX_DURABLE_STREAMS_PER_SESSION
    );
    assert_eq!(
        index.apply_session_references(None, &record(mapping(MAX_DURABLE_STREAMS_PER_SESSION))),
        Err(StreamStoreError::StreamLimit)
    );
}

#[test]
async fn session_control_batch_validates_before_appending_any_record() {
    use crate::services::oplog::OplogOps;
    use golem_common::model::durable_stream::{
        StreamSessionCancelRequestedRecord, StreamSlotTombstonedRecord,
    };

    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let cancel = StreamSessionRecord::CancelRequested(StreamSessionCancelRequestedRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: identity.invocation.clone(),
    });
    let tombstone = StreamSessionRecord::Tombstoned(StreamSlotTombstonedRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: identity.invocation,
        slot: "$result".into(),
        role: SessionStreamRole::Output,
    });
    let mut malformed = tombstone.clone();
    if let StreamSessionRecord::Tombstoned(record) = &mut malformed {
        record.slot.clear();
    }
    let before = oplog.current_oplog_index().await;
    let invalid_records = vec![cancel.clone(), malformed];
    assert!(matches!(
        producer
            .run_owned(None, 0, move |owner, context| async move {
                owner
                    .append_session_records_owned(&context, None, invalid_records)
                    .await
            })
            .await,
        Err(StreamStoreError::CorruptHistory(_))
    ));
    assert_eq!(oplog.current_oplog_index().await, before);

    let records = vec![cancel.clone(), tombstone.clone()];
    producer
        .run_owned(None, 0, move |owner, context| async move {
            owner
                .append_session_records_owned(&context, None, records)
                .await
        })
        .await
        .unwrap();
    assert_eq!(
        oplog.current_oplog_index().await.as_u64(),
        before.as_u64() + 2
    );
    for (offset, expected) in [cancel, tombstone].into_iter().enumerate() {
        let entry = oplog
            .read(OplogIndex::from_u64(before.as_u64() + 1 + offset as u64))
            .await;
        let OplogEntry::StreamSession { record, .. } = entry else {
            panic!("expected session control record");
        };
        assert_eq!(oplog.download_payload(record).await.unwrap(), expected);
    }
}

#[test]
async fn malformed_session_record_is_rejected_at_the_write_boundary() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let mut malformed_handle = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    malformed_handle.format_version = DURABLE_STREAM_FORMAT_VERSION + 1;
    let before = oplog.current_oplog_index().await;

    assert!(matches!(
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation,
                    mapping: StreamSessionMappingRecord {
                        transport_stream_id: 17,
                        handle: malformed_handle,
                        role: SessionStreamRole::Output,
                    },
                },)
            )
            .await,
        Err(StreamStoreError::CorruptHistory(_))
    ));
    assert_eq!(oplog.current_oplog_index().await, before);
}

#[test]
async fn recursive_value_limit_commits_only_one_protocol_resource_exhausted_terminal() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog.clone(), &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let stream_id = registered.value.stream_id;
    let nested = (0..=MAX_NEW_STREAM_HANDLES_PER_VALUE)
        .map(|position| {
            registration(
                &identity,
                StreamRegistrationCoordinate::Nested {
                    parent_stream_id: stream_id,
                    parent_producer_sequence: 0,
                    recursive_value_path: vec![StreamValuePathStep::ListElement(position as u32)],
                },
                StreamSourceKind::Nested,
            )
        })
        .collect();

    assert_eq!(
        producer
            .write_items_with_nested(
                None,
                stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1]]),
                nested,
            )
            .await,
        Err(StreamStoreError::ValueStreamLimit)
    );
    assert_eq!(oplog.committed_length(), 2);
    assert_eq!(producer.index.lock().await.registrations.len(), 1);

    let mut reader = producer.catch_up(registered.value, None).await.unwrap();
    let terminal = reader.next().await.unwrap().unwrap();
    assert_eq!(terminal.producer_sequence, 0);
    assert_eq!(
        terminal.terminal_author,
        Some(StreamTerminalAuthor::Protocol)
    );
    let CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(bytes)) =
        terminal.payload
    else {
        panic!("expected resource exhaustion stream terminal")
    };
    let error: AgentError = golem_common::serialization::deserialize(&bytes).unwrap();
    assert_eq!(error.to_string(), "\"ResourceExhausted\"");

    assert!(matches!(
        producer
            .write_items(
                None,
                stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![2]]),
            )
            .await,
        Err(StreamStoreError::FencedByTerminal(_))
    ));
    assert_eq!(oplog.committed_length(), 2);
}

#[test]
async fn traversal_session_and_counter_limits_terminalize_without_partial_items() {
    async fn fresh() -> (
        TestIdentity,
        Arc<TestOplog>,
        Arc<DurableStreamStore>,
        golem_common::base_model::durable_stream::DurableStreamHandle,
    ) {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(None, root_registration(&identity))
            .await
            .unwrap()
            .value;
        (identity, oplog, producer, handle)
    }

    let (_identity, depth_oplog, depth_producer, depth_handle) = fresh().await;
    assert_eq!(
        depth_producer
            .write_items_with_nested_at_depth(
                None,
                depth_handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1]]),
                Vec::new(),
                MAX_STREAM_VALUE_TRAVERSAL_DEPTH + 1,
            )
            .await,
        Err(StreamStoreError::TraversalDepthLimit)
    );
    assert_eq!(depth_oplog.committed_length(), 2);

    let (identity, stream_oplog, stream_producer, stream_handle) = fresh().await;
    {
        let mut index = stream_producer.index.lock().await;
        let session_key = index
            .stream_sessions
            .get(&stream_handle.stream_id)
            .unwrap()
            .clone();
        index
            .session_stream_counts
            .insert(session_key, MAX_DURABLE_STREAMS_PER_SESSION);
    }
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: stream_handle.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: vec![StreamValuePathStep::OptionSome],
        },
        StreamSourceKind::Nested,
    );
    assert_eq!(
        stream_producer
            .write_items_with_nested(
                None,
                stream_handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1]]),
                vec![nested],
            )
            .await,
        Err(StreamStoreError::StreamLimit)
    );
    assert_eq!(stream_oplog.committed_length(), 2);

    let (_identity, counter_oplog, counter_producer, counter_handle) = fresh().await;
    counter_producer
        .index
        .lock()
        .await
        .streams
        .get_mut(&counter_handle.stream_id)
        .unwrap()
        .next_sequence = u64::MAX;
    assert_eq!(
        counter_producer
            .write_items(
                None,
                counter_handle.stream_id,
                u64::MAX,
                StreamItemsPayload::Values(vec![vec![1]]),
            )
            .await,
        Err(StreamStoreError::CounterOverflow)
    );
    assert_eq!(counter_oplog.committed_length(), 2);
    let mut reader = counter_producer
        .catch_up(counter_handle, None)
        .await
        .unwrap();
    assert_eq!(
        reader.next().await.unwrap().unwrap().producer_sequence,
        u64::MAX
    );
}

#[test]
#[timeout("30s")]
async fn restart_recovers_registration_committed_before_caller_observation() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let commit_reached = Arc::new(Barrier::new(2));
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let commit_reached = commit_reached.clone();
        move |committed| {
            let oplog = oplog.clone();
            let commit_reached = commit_reached.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if let Some(committed) = committed {
                    let _ = committed.send(());
                }
                commit_reached.wait().await;
                std::future::pending::<()>().await;
            })
        }
    });
    let live = DurableStreamStore::load_with_commit(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let request = root_registration(&identity);
    let registration = tokio::spawn({
        let producer = live.clone();
        let request = request.clone();
        async move { producer.register(None, request).await }
    });

    commit_reached.wait().await;
    assert_eq!(oplog.committed_length(), 1);
    assert!(!registration.is_finished());
    registration.abort();
    registration.await.unwrap_err();
    drop(live);

    let restarted = producer(oplog.clone(), &identity, None).await;
    let handle = restarted.validate_registration(&request).await.unwrap();
    assert_eq!(
        handle.stream_id,
        StreamId::derive(
            identity.environment_id,
            &identity.agent_id,
            identity.fingerprint,
            OplogIndex::INITIAL,
        )
        .unwrap()
    );
    assert_eq!(oplog.committed_length(), 1);
    assert!(restarted.register(None, request).await.unwrap().replayed);
    assert_eq!(oplog.committed_length(), 1);
}

#[test]
#[timeout("30s")]
async fn remote_cancellation_releases_durable_activity_but_retains_owned_admission() {
    for abandon in [false, true] {
        let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
        let (started, ready) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let caller = tokio::spawn({
            let live = live.clone();
            async move {
                live.run_admitted(None, 7, true, move |_, admission| async move {
                    admission
                        .submit(|_, _| async { Ok::<(), StreamStoreError>(()) })
                        .await?;
                    started.send(()).unwrap();
                    released.await.unwrap();
                    assert_eq!(
                        admission
                            .submit(|_, _| async { Ok::<(), StreamStoreError>(()) })
                            .await,
                        Err(StreamStoreError::RecoveryRequired)
                    );
                    Err::<(), _>(StreamStoreError::Oplog("remote failure".into()))
                })
                .await
            }
        });
        ready.await.unwrap();
        assert!(!caller.is_finished());
        live.durable_activity.close();
        tokio::time::timeout(Duration::from_secs(5), live.wait_durable_drained())
            .await
            .expect("remote RPC retained local durable activity");
        assert_eq!(live.lifecycle_operations.available_permits(), 15);
        assert_eq!(
            live.lifecycle_operation_bytes.available_permits(),
            256 * 1024 * 1024 - 7
        );
        if abandon {
            caller.abort();
        }
        release.send(()).unwrap();
        if abandon {
            assert!(caller.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(
                caller.await.unwrap(),
                Err(StreamStoreError::Oplog("remote failure".into()))
            );
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while live.lifecycle_operations.available_permits() != 16 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            live.lifecycle_operation_bytes.available_permits(),
            256 * 1024 * 1024
        );
        live.ensure_healthy().unwrap();
    }
}

#[test]
#[timeout("30s")]
async fn session_notification_waits_for_status_fold_after_caller_cancellation() {
    let identity = identity();
    let release = Arc::new(Notify::new());
    let folded = Arc::new(AtomicBool::new(false));
    let commit: DurableStreamCommit = Arc::new({
        let release = release.clone();
        let folded = folded.clone();
        move |receipt| {
            let release = release.clone();
            let folded = folded.clone();
            Box::pin(async move {
                receipt.unwrap().send(()).unwrap();
                release.notified().await;
                folded.store(true, Ordering::Release);
            })
        }
    });
    let live = DurableStreamStore::load_with_commit(
        Arc::new(TestOplog::default()),
        identity.environment_id,
        identity.agent_id,
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let mut notification = Box::pin(live.session_records_changed().notified());
    notification.as_mut().enable();
    let (requested, ready) = oneshot::channel();
    let caller = tokio::spawn({
        let live = live.clone();
        async move {
            live.run_owned(None, 0, move |owner, context| async move {
                owner.commit(&context).await;
                context.finish_durable_effect();
                owner.notify_session_records_changed(Some(&context));
                requested.send(()).unwrap();
                Ok::<(), StreamStoreError>(())
            })
            .await
        }
    });
    ready.await.unwrap();
    assert!(futures::poll!(notification.as_mut()).is_pending());
    assert!(!folded.load(Ordering::Acquire));
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.notify_one();
    notification.await;
    assert!(folded.load(Ordering::Acquire));
    live.wait_durable_drained().await;
}

#[test]
#[timeout("30s")]
async fn durable_activity_waits_for_callback_tails_but_not_abandoned_fanout() {
    for lifecycle in [false, true] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let block = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let block = block.clone();
            let committed = committed.clone();
            let release = release.clone();
            move |receipt| {
                let oplog = oplog.clone();
                let block = block.clone();
                let committed = committed.clone();
                let release = release.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(receipt) = receipt {
                        let _ = receipt.send(());
                    }
                    if block.swap(false, Ordering::AcqRel) {
                        committed.notify_one();
                        release.notified().await;
                    }
                })
            }
        });
        let live = DurableStreamStore::load_with_commit(
            oplog,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            Some(1),
            commit,
        )
        .await
        .unwrap();
        let handle = live
            .register(None, root_registration(&identity))
            .await
            .unwrap()
            .value;
        let mut reader = live
            .bus(handle.stream_id)
            .unwrap()
            .subscribe()
            .await
            .unwrap();
        live.write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![3]]),
        )
        .await
        .unwrap();
        block.store(true, Ordering::Release);
        let caller = tokio::spawn({
            let live = live.clone();
            async move {
                if lifecycle {
                    live.end_open(None, handle.stream_id, StreamEndResult::Ok)
                        .await
                } else {
                    live.write_items(
                        None,
                        handle.stream_id,
                        1,
                        StreamItemsPayload::Values(vec![vec![7]]),
                    )
                    .await
                    .map(|_| ())
                }
            }
        });
        committed.notified().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        live.durable_activity.close();
        let drained = live.durable_activity.wait_drained();
        tokio::pin!(drained);
        assert!(futures::poll!(&mut drained).is_pending());
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), &mut drained)
            .await
            .expect("durable activity retained a blocked publication");
        assert_eq!(
            live.run_owned(None, 0, |_, _| async { Ok::<(), StreamStoreError>(()) })
                .await,
            Err(StreamStoreError::RecoveryRequired)
        );
        if !lifecycle {
            assert_eq!(live.owned_operations.available_permits(), 15);
        }
        let first = reader.recv().await.unwrap();
        let second = reader.recv().await.unwrap();
        assert_eq!(
            first.payload.payload,
            CommittedProducerStreamEventPayload::Value(vec![3])
        );
        assert!(first.offset < second.offset);
        assert_eq!(
            second.payload.payload,
            if lifecycle {
                CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
            } else {
                CommittedProducerStreamEventPayload::Value(vec![7])
            }
        );
    }
}

#[test]
#[timeout("30s")]
async fn lifecycle_cancellations_outlive_callers_under_saturated_data_admission() {
    for saturate_bytes in [false, true] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, Some(1)).await;
        let mut streams = Vec::new();
        // More blocked terminals than the lifecycle lane can admit at once, and more
        // buses than one dispatcher page, exercise both admission and scan progress.
        for i in 0..33 {
            let handle = live
                .register(
                    None,
                    registration(
                        &identity,
                        StreamRegistrationCoordinate::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKind::MethodResult,
                            recursive_value_path: vec![StreamValuePathStep::TupleElement(i)],
                        },
                        StreamSourceKind::InvocationOutput,
                    ),
                )
                .await
                .unwrap()
                .value;
            let reader = live
                .bus(handle.stream_id)
                .unwrap()
                .subscribe()
                .await
                .unwrap();
            live.write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![3]]),
            )
            .await
            .unwrap();
            streams.push((handle.stream_id, reader));
        }
        let blocked_count = if saturate_bytes { 1 } else { 16 };
        let mut writes = Vec::new();
        for (stream_id, _) in streams.iter().take(blocked_count) {
            let stream_id = *stream_id;
            let live = live.clone();
            writes.push(tokio::spawn(async move {
                // Charge the whole byte lane without allocating a 256 MiB test payload.
                // The nested write retains that reservation through blocked delivery.
                live.run_owned(
                    None,
                    if saturate_bytes { 256 * 1024 * 1024 } else { 1 },
                    move |owner, context| async move {
                        owner
                            .write_items(
                                Some(&context),
                                stream_id,
                                1,
                                StreamItemsPayload::Values(vec![vec![7]]),
                            )
                            .await
                    },
                )
                .await
            }));
        }
        while oplog.committed_length() < (66 + blocked_count) as u64 {
            tokio::task::yield_now().await;
        }
        if saturate_bytes {
            assert_eq!(live.owned_operation_bytes.available_permits(), 0);
        } else {
            assert_eq!(live.owned_operations.available_permits(), 0);
        }
        let mut cancellations = Vec::new();
        for (stream_id, _) in &streams {
            let stream_id = *stream_id;
            let live = live.clone();
            cancellations.push(tokio::spawn(async move {
                live.cancel_open(
                    None,
                    stream_id,
                    StreamCancelRole::OutputConsumer,
                    StreamCancelReason::Cancelled,
                    None,
                )
                .await
            }));
        }
        loop {
            let index = live.index.lock().await;
            let committed = index
                .streams
                .values()
                .filter(|stream| stream.terminal)
                .count();
            drop(index);
            if committed == 33 && live.lifecycle_operations.available_permits() == 16 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(writes.iter().all(|write| !write.is_finished()));
        assert!(cancellations.iter().all(|cancel| !cancel.is_finished()));
        for cancel in cancellations.iter().step_by(2) {
            cancel.abort();
        }
        for (i, (_, mut reader)) in streams.into_iter().enumerate() {
            let first = reader.recv().await.unwrap();
            assert_eq!(
                first.payload.payload,
                CommittedProducerStreamEventPayload::Value(vec![3])
            );
            let mut previous = first.offset;
            if i < blocked_count {
                let second = reader.recv().await.unwrap();
                assert_eq!(
                    second.payload.payload,
                    CommittedProducerStreamEventPayload::Value(vec![7])
                );
                assert!(previous < second.offset);
                previous = second.offset;
            }
            let terminal = reader.recv().await.unwrap();
            assert!(previous < terminal.offset);
            assert_eq!(
                terminal.payload.payload,
                CommittedProducerStreamEventPayload::Cancel {
                    role: StreamCancelRole::OutputConsumer,
                    reason: StreamCancelReason::Cancelled,
                    details: None,
                }
            );
        }
        for write in writes {
            write.await.unwrap().unwrap();
        }
        for (i, cancel) in cancellations.into_iter().enumerate() {
            if i % 2 == 0 {
                assert!(cancel.await.unwrap_err().is_cancelled());
            } else {
                cancel.await.unwrap().unwrap();
            }
        }
        assert_eq!(live.owned_operations.available_permits(), 16);
        assert_eq!(
            live.owned_operation_bytes.available_permits(),
            256 * 1024 * 1024
        );
    }
}

#[test]
#[timeout("30s")]
async fn session_finish_reserves_repeated_terminal_errors_before_appending() {
    use crate::services::oplog::OplogOps;
    use golem_common::base_model::durable_stream::StreamSessionFinishedRecord;

    for error_len in [128 * 1024, 2 * 1024 * 1024] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let mut handles = Vec::new();
        for i in 0..3 {
            handles.push(
                live.register(
                    None,
                    registration(
                        &identity,
                        StreamRegistrationCoordinate::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKind::MethodResult,
                            recursive_value_path: vec![StreamValuePathStep::TupleElement(i)],
                        },
                        StreamSourceKind::InvocationOutput,
                    ),
                )
                .await
                .unwrap()
                .value,
            );
        }
        let before = oplog.current_oplog_index().await;
        let occupied = live
            .lifecycle_operation_bytes
            .clone()
            .acquire_many_owned(256 * 1024 * 1024 - 128)
            .await
            .unwrap();
        let error = vec![71; error_len];
        let mut finishing = Box::pin(live.finish_session(
            None,
            identity.invocation.clone(),
            None,
            Err(error.clone()),
            StreamCancelReason::InvocationFailed,
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut finishing)
                .await
                .is_err()
        );
        assert_eq!(oplog.current_oplog_index().await, before);
        drop(occupied);
        finishing.await.unwrap();
        let finished_index = oplog.current_oplog_index().await;
        assert_eq!(finished_index.as_u64(), before.as_u64() + 4);
        let OplogEntry::StreamSession { record, .. } = oplog.read(finished_index).await else {
            panic!("terminal batch must end with Finished");
        };
        assert_eq!(
            oplog.download_payload(record).await.unwrap(),
            StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                result: Err(error.clone()),
            })
        );
        assert!(
            live.index
                .lock()
                .await
                .streams
                .values()
                .all(|stream| stream.terminal_event.is_none())
        );
        let restarted = producer(oplog.clone(), &identity, None).await;
        for current in [&live, &restarted] {
            for handle in &handles {
                let mut reader = current.catch_up(handle.clone(), None).await.unwrap();
                assert_eq!(
                    reader.next().await.unwrap().unwrap().payload,
                    CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(
                        error.clone()
                    ))
                );
            }
            let occupied = current
                .lifecycle_operation_bytes
                .clone()
                .acquire_many_owned(256 * 1024 * 1024)
                .await
                .unwrap();
            tokio::time::timeout(
                Duration::from_secs(1),
                current.finish_session(
                    None,
                    identity.invocation.clone(),
                    None,
                    Err(vec![99; error_len]),
                    StreamCancelReason::InvocationFailed,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(oplog.current_oplog_index().await, finished_index);
            drop(occupied);
        }
        assert_eq!(
            live.lifecycle_operation_bytes.available_permits(),
            256 * 1024 * 1024
        );
    }
}

#[test]
#[timeout("30s")]
async fn session_finish_reserves_batch_memory_for_maximum_stream_count() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live = producer(oplog.clone(), &identity, None).await;
    for i in 0..MAX_DURABLE_STREAMS_PER_SESSION {
        live.register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(i as u32)],
                },
                StreamSourceKind::InvocationOutput,
            ),
        )
        .await
        .unwrap();
    }
    let before = oplog.current_oplog_index().await;
    let occupied = live
        .lifecycle_operation_bytes
        .clone()
        .acquire_many_owned(256 * 1024 * 1024 - 1024)
        .await
        .unwrap();
    let mut finishing = Box::pin(live.finish_session(
        None,
        identity.invocation.clone(),
        None,
        Err(vec![83; 128]),
        StreamCancelReason::InvocationFailed,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut finishing)
            .await
            .is_err()
    );
    assert_eq!(oplog.current_oplog_index().await, before);
    drop(occupied);
    finishing.await.unwrap();
    assert_eq!(
        oplog.current_oplog_index().await.as_u64(),
        before.as_u64() + MAX_DURABLE_STREAMS_PER_SESSION as u64 + 1
    );
    assert!(
        live.index
            .lock()
            .await
            .streams
            .values()
            .all(|stream| stream.terminal && stream.terminal_event.is_none())
    );

    live.run_lifecycle(None, 256 * 1024 * 1024 + 1, |owner, context| async move {
        assert_eq!(owner.lifecycle_operation_bytes.available_permits(), 0);
        context
            .run_nested(|_, _| async { Ok::<(), StreamStoreError>(()) })
            .await
    })
    .await
    .unwrap();
    let released = live
        .lifecycle_operation_bytes
        .clone()
        .acquire_many_owned(256 * 1024 * 1024)
        .await
        .unwrap();
    drop(released);
    assert_eq!(
        live.lifecycle_operation_bytes.available_permits(),
        256 * 1024 * 1024
    );
}

#[test]
#[timeout("30s")]
async fn multistream_lifecycle_batches_release_admission_before_delivery() {
    for deleting in [false, true] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, Some(1)).await;
        let mut readers = Vec::new();
        for i in 0..3 {
            let handle = live
                .register(
                    None,
                    registration(
                        &identity,
                        StreamRegistrationCoordinate::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKind::MethodResult,
                            recursive_value_path: vec![StreamValuePathStep::TupleElement(i)],
                        },
                        StreamSourceKind::InvocationOutput,
                    ),
                )
                .await
                .unwrap()
                .value;
            let reader = live
                .bus(handle.stream_id)
                .unwrap()
                .subscribe()
                .await
                .unwrap();
            live.write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![i as u8 + 3]]),
            )
            .await
            .unwrap();
            readers.push(reader);
        }
        let finishing = tokio::spawn({
            let live = live.clone();
            let session = identity.invocation.clone();
            async move {
                if deleting {
                    live.commit_deletion_barrier(123, true).await
                } else {
                    live.finish_session(
                        None,
                        session,
                        None,
                        Err(vec![19, 31]),
                        StreamCancelReason::InvocationFailed,
                    )
                    .await
                }
            }
        });
        loop {
            let index = live.index.lock().await;
            let committed = if deleting {
                index.deleting
            } else {
                index.finished_sessions.contains(&identity.invocation)
            };
            drop(index);
            if committed && live.lifecycle_operations.available_permits() == 16 {
                break;
            }
            tokio::task::yield_now().await;
        }
        if deleting {
            tokio::time::timeout(Duration::from_secs(5), finishing)
                .await
                .expect("deletion waited for terminal delivery")
                .unwrap()
                .unwrap();
        } else {
            assert!(!finishing.is_finished());
            finishing.abort();
            assert!(finishing.await.unwrap_err().is_cancelled());
        }
        for (i, mut reader) in readers.into_iter().enumerate() {
            let item = reader.recv().await.unwrap();
            assert_eq!(
                item.payload.payload,
                CommittedProducerStreamEventPayload::Value(vec![i as u8 + 3])
            );
            let terminal = reader.recv().await.unwrap();
            assert!(item.offset < terminal.offset);
            if deleting {
                assert!(matches!(
                    terminal.payload.payload,
                    CommittedProducerStreamEventPayload::Cancel {
                        reason: StreamCancelReason::ProducerDeleting,
                        ..
                    }
                ));
            } else {
                assert_eq!(
                    terminal.payload.payload,
                    CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(vec![
                        19, 31
                    ]))
                );
            }
        }
        let restarted = producer(oplog, &identity, None).await;
        let index = restarted.index.lock().await;
        assert_eq!(
            index
                .streams
                .values()
                .filter(|stream| stream.terminal)
                .count(),
            3
        );
        assert_eq!(index.deleting, deleting);
        assert_eq!(
            index.finished_sessions.contains(&identity.invocation),
            !deleting
        );
    }
}

#[test]
#[timeout("30s")]
async fn commit_completes_before_backpressured_item_and_terminal_fanout() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live_producer = producer(oplog.clone(), &identity, Some(1)).await;
    let registered = live_producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let mut reader = live_producer
        .catch_up(registered.value.clone(), None)
        .await
        .unwrap();
    live_producer
        .write_items(
            None,
            registered.value.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1]]),
        )
        .await
        .unwrap();

    let blocked_item = tokio::spawn({
        let producer = live_producer.clone();
        let stream_id = registered.value.stream_id;
        async move {
            producer
                .write_items(
                    None,
                    stream_id,
                    1,
                    StreamItemsPayload::Values(vec![vec![2]]),
                )
                .await
        }
    });
    while oplog.committed_length() < 3 {
        tokio::task::yield_now().await;
    }
    assert!(!blocked_item.is_finished());
    assert!(oplog.commit_count() >= 3);
    assert_eq!(reader.next().await.unwrap().unwrap().producer_sequence, 0);
    blocked_item.await.unwrap().unwrap();

    let blocked_terminal = tokio::spawn({
        let producer = live_producer.clone();
        let stream_id = registered.value.stream_id;
        async move { producer.end(None, stream_id, 2, StreamEndResult::Ok).await }
    });
    while oplog.committed_length() < 4 {
        tokio::task::yield_now().await;
    }
    assert!(!blocked_terminal.is_finished());
    drop(reader);
    blocked_terminal.await.unwrap().unwrap();

    let restarted = producer(oplog.clone(), &identity, None).await;
    let mut catch_up = restarted.catch_up(registered.value, None).await.unwrap();
    assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 0);
    assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 1);
    assert!(matches!(
        catch_up.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));
}

#[test]
#[timeout("30s")]
async fn historical_catch_up_does_not_deadlock_with_backpressured_publication() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live_producer = producer(oplog.clone(), &identity, Some(1)).await;
    let handle = live_producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    live_producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![0]]),
        )
        .await
        .unwrap();

    let bus = live_producer.bus(handle.stream_id).unwrap();
    let mut subscription = bus.subscribe().await.unwrap();
    let join_high_water = subscription.high_water;
    live_producer
        .write_items(
            None,
            handle.stream_id,
            1,
            StreamItemsPayload::Values(vec![vec![1]]),
        )
        .await
        .unwrap();
    let blocked_publication = tokio::spawn({
        let producer = live_producer.clone();
        let stream_id = handle.stream_id;
        async move {
            producer
                .write_items(
                    None,
                    stream_id,
                    2,
                    StreamItemsPayload::Values(vec![vec![2]]),
                )
                .await
        }
    });
    while oplog.committed_length() < 4 {
        tokio::task::yield_now().await;
    }
    assert!(!blocked_publication.is_finished());

    let history = tokio::time::timeout(
        Duration::from_secs(1),
        live_producer.read_segment(&handle, None, join_high_water),
    )
    .await
    .expect("historical catch-up must not wait for bounded live fanout")
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].producer_sequence, 0);

    assert_eq!(
        subscription.recv().await.unwrap().payload.producer_sequence,
        1
    );
    blocked_publication.await.unwrap().unwrap();
    assert_eq!(
        subscription.recv().await.unwrap().payload.producer_sequence,
        2
    );
}

#[test]
#[timeout("30s")]
async fn restart_recovers_an_item_and_terminal_after_commit_before_fanout() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live_producer = producer(oplog.clone(), &identity, Some(1)).await;
    let registered = live_producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let blocked_reader = live_producer
        .catch_up(registered.value.clone(), None)
        .await
        .unwrap();
    live_producer
        .write_items(
            None,
            registered.value.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![0]]),
        )
        .await
        .unwrap();

    let blocked_item = tokio::spawn({
        let producer = live_producer.clone();
        let stream_id = registered.value.stream_id;
        async move {
            producer
                .write_items(
                    None,
                    stream_id,
                    1,
                    StreamItemsPayload::Values(vec![vec![1]]),
                )
                .await
        }
    });
    while oplog.committed_length() < 3 {
        tokio::task::yield_now().await;
    }
    assert!(!blocked_item.is_finished());
    blocked_item.abort();
    blocked_item.await.unwrap_err();

    let blocked_terminal = tokio::spawn({
        let producer = live_producer.clone();
        let stream_id = registered.value.stream_id;
        async move { producer.end(None, stream_id, 2, StreamEndResult::Ok).await }
    });
    while oplog.committed_length() < 4 {
        tokio::task::yield_now().await;
    }
    assert!(!blocked_terminal.is_finished());
    blocked_terminal.abort();
    blocked_terminal.await.unwrap_err();
    drop(blocked_reader);
    drop(live_producer);

    let restarted = producer(oplog, &identity, None).await;
    let mut catch_up = restarted.catch_up(registered.value, None).await.unwrap();
    assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 0);
    assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 1);
    assert!(matches!(
        catch_up.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));
}

#[test]
async fn rejected_catch_up_cursor_does_not_consume_live_reader_capacity() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    producer
        .write_items(
            None,
            registered.value.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1]]),
        )
        .await
        .unwrap();
    let unavailable_cursor = StreamOffset::new(OplogIndex::from_u64(999), 0);

    for _ in 0..golem_common::base_model::durable_stream::MAX_LIVE_READERS_PER_STREAM {
        assert!(matches!(
            producer
                .catch_up(registered.value.clone(), Some(unavailable_cursor))
                .await,
            Err(StreamStoreError::CursorUnavailable)
        ));
    }

    producer
        .catch_up(registered.value, None)
        .await
        .expect("rejected admissions must not consume live-reader capacity");
}

#[test]
async fn unavailable_cursor_is_rejected_for_an_empty_stream() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    let unavailable_cursor = StreamOffset::new(OplogIndex::from_u64(999), 0);

    assert!(matches!(
        producer
            .catch_up(registered.value, Some(unavailable_cursor))
            .await,
        Err(StreamStoreError::CursorUnavailable)
    ));
}

#[test]
async fn terminal_delivery_does_not_wait_for_live_reader_cleanup() {
    let bus = Arc::new(DurableLiveStreamBus::new(2).unwrap());
    let subscription = bus.subscribe().await.unwrap();
    let stream_id = StreamId(Uuid::from_u128(99));
    let mut reader = DurableCatchUpReader {
        bus: bus.clone(),
        subscription: Some(subscription),
        history_source: None,
        history: VecDeque::from([
            CommittedProducerStreamEvent {
                stream_id,
                producer_sequence: 0,
                offset: StreamOffset::new(OplogIndex::from_u64(1), 0),
                packed_u8_batch_end: Some(StreamOffset::new(OplogIndex::from_u64(1), 0)),
                terminal_author: None,
                nested_handles: Vec::new(),
                payload: CommittedProducerStreamEventPayload::PackedU8(7),
            },
            CommittedProducerStreamEvent {
                stream_id,
                producer_sequence: 1,
                offset: StreamOffset::new(OplogIndex::from_u64(2), 0),
                packed_u8_batch_end: None,
                terminal_author: None,
                nested_handles: Vec::new(),
                payload: CommittedProducerStreamEventPayload::End(StreamEndResult::Ok),
            },
        ]),
        join_high_water: None,
        last_delivered: None,
        terminal_delivered: false,
    };
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::PackedU8(7)
    ));

    let (acquired_tx, acquired_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let locked_bus = bus.clone();
    let lock_task = tokio::spawn(async move {
        locked_bus
            .hold_state_lock_until(acquired_tx, release_rx)
            .await;
    });
    acquired_rx.await.unwrap();

    let terminal = tokio::time::timeout(Duration::from_millis(100), reader.next())
        .await
        .expect("terminal delivery waited for live-reader cleanup")
        .unwrap()
        .unwrap();
    assert!(matches!(
        terminal.payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));
    release_tx.send(()).unwrap();
    lock_task.await.unwrap();
}

#[test]
async fn poisoned_producer_rejects_prefetched_history_without_advancing_cursor() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    producer
        .write_items(
            None,
            registered.value.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![3, 7]),
        )
        .await
        .unwrap();
    let mut reader = producer.catch_up(registered.value, None).await.unwrap();
    assert!(!reader.history.is_empty());
    producer.poison();
    assert_eq!(reader.next().await, Err(StreamStoreError::RecoveryRequired));
    assert_eq!(reader.next().await, Err(StreamStoreError::RecoveryRequired));
    assert_eq!(reader.last_delivered, None);
    assert!(!reader.terminal_delivered);
}

#[test]
async fn completed_terminal_catch_up_reader_releases_live_reader_capacity() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let registered = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();
    producer
        .end(None, registered.value.stream_id, 0, StreamEndResult::Ok)
        .await
        .unwrap();

    let mut completed = producer
        .catch_up(registered.value.clone(), None)
        .await
        .unwrap();
    assert!(matches!(
        completed.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));
    assert!(completed.next().await.unwrap().is_none());

    let mut active = Vec::new();
    for _ in 0..golem_common::base_model::durable_stream::MAX_LIVE_READERS_PER_STREAM {
        active.push(
            producer
                .catch_up(registered.value.clone(), None)
                .await
                .expect("a completed terminal reader is no longer a simultaneous live reader"),
        );
    }
}

#[test]
async fn nested_registration_must_match_its_enclosing_stream_coordinate() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = producer(oplog, &identity, None).await;
    let enclosing = producer
        .register(None, root_registration(&identity))
        .await
        .unwrap();

    let mut other_session = identity.invocation.clone();
    other_session.idempotency_key = IdempotencyKey::new("other-session".to_string());
    let other = producer
        .register(
            None,
            ProducerRegistrationRequest {
                coordinate: StreamRegistrationCoordinate::Root {
                    invocation_id: other_session.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: other_session,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKind::InvocationOutput,
                session_mapping: None,
                entity_parent_start_index: None,
            },
        )
        .await
        .unwrap();
    let nested_with_wrong_parent = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: other.value.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: vec![StreamValuePathStep::OptionSome],
        },
        StreamSourceKind::Nested,
    );

    assert_eq!(
        producer
            .write_items_with_nested(
                None,
                enclosing.value.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1]]),
                vec![nested_with_wrong_parent],
            )
            .await,
        Err(StreamStoreError::RegistrationDivergence)
    );
}

#[test]
async fn session_finish_serializes_with_nested_topology_and_fences_later_events() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let live_producer = producer(oplog.clone(), &identity, None).await;
    let root = live_producer
        .register(None, root_registration(&identity))
        .await
        .unwrap()
        .value;
    let nested = registration(
        &identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: root.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: vec![StreamValuePathStep::OptionSome],
        },
        StreamSourceKind::Nested,
    );

    let writing = {
        let producer = live_producer.clone();
        tokio::spawn(async move {
            producer
                .write_items_with_nested(
                    None,
                    root.stream_id,
                    0,
                    StreamItemsPayload::Values(vec![vec![1]]),
                    vec![nested],
                )
                .await
        })
    };
    let finishing = {
        let producer = live_producer.clone();
        let session_key = identity.invocation.clone();
        tokio::spawn(async move {
            producer
                .finish_session(
                    None,
                    session_key,
                    None,
                    Err(b"failed".to_vec()),
                    golem_common::base_model::durable_stream::StreamCancelReason::InvocationFailed,
                )
                .await
        })
    };
    let write_result = writing.await.unwrap();
    finishing.await.unwrap().unwrap();
    assert!(
        write_result.is_ok() || matches!(&write_result, Err(StreamStoreError::SessionFinished(_)))
    );
    assert!(matches!(
        live_producer
            .write_items(
                None,
                root.stream_id,
                usize::from(write_result.is_ok()) as u64,
                StreamItemsPayload::Values(vec![vec![2]]),
            )
            .await,
        Err(StreamStoreError::SessionFinished(_))
    ));

    let entries = oplog.entries();
    let OplogEntry::StreamSession {
        record: OplogPayload::Inline(record),
        ..
    } = entries.last().expect("session finish entry is missing")
    else {
        panic!("session finish must be the last committed entry");
    };
    assert!(matches!(record.as_ref(), StreamSessionRecord::Finished(_)));

    drop(live_producer);
    let restarted = producer(oplog, &identity, None).await;
    assert!(matches!(
        restarted
            .write_items(
                None,
                root.stream_id,
                usize::from(write_result.is_ok()) as u64,
                StreamItemsPayload::Values(vec![vec![2]]),
            )
            .await,
        Err(StreamStoreError::SessionFinished(_))
    ));
}
