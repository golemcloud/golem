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
use crate::services::oplog::multilayer::{
    MultiLayerOplogService, OplogArchiveResult, OplogArchiveService, new_transfer_fiber,
};
use crate::services::oplog::primary::PrimaryOplogService;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use async_trait::async_trait;
use golem_common::model::AgentFingerprint;
use golem_common::model::RetryConfig;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use nonempty_collections::nev;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};
use tokio::sync::{Notify, Semaphore};

pub struct GatedArchive {
    entries: Mutex<BTreeMap<OplogIndex, OplogEntry>>,
    append_started: Notify,
    append_permits: Semaphore,
    append_calls: AtomicUsize,
    read_calls: AtomicUsize,
    /// Once set, appends are refused with this fence and write nothing, as a compressed layer
    /// refuses them after a newer owner recorded its epoch. The archive does not report it through
    /// `fence`, so only the writer task's latch can tell the oplog.
    refusal: Mutex<Option<OplogFence>>,
}

impl Default for GatedArchive {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            append_started: Notify::new(),
            append_permits: Semaphore::new(0),
            append_calls: AtomicUsize::new(0),
            read_calls: AtomicUsize::new(0),
            refusal: Mutex::default(),
        }
    }
}

impl Debug for GatedArchive {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatedArchive").finish()
    }
}

impl GatedArchive {
    pub async fn wait_for_appends(&self, count: usize) {
        while self.append_calls.load(Ordering::Acquire) < count {
            self.append_started.notified().await;
        }
    }

    pub fn release(&self, count: usize) {
        self.append_permits.add_permits(count);
    }
}

#[async_trait]
impl OplogArchive for GatedArchive {
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> OplogArchiveResult<BTreeMap<OplogIndex, OplogEntry>> {
        self.read_calls.fetch_add(1, Ordering::Relaxed);
        let end = idx.as_u64().saturating_add(n);
        Ok(self
            .entries
            .lock()
            .unwrap()
            .range(idx..OplogIndex::from_u64(end))
            .map(|(idx, entry)| (*idx, entry.clone()))
            .collect())
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> Result<u64, OplogError> {
        self.append_calls.fetch_add(1, Ordering::Release);
        self.append_started.notify_one();
        self.append_permits
            .acquire()
            .await
            .map_err(|error| OplogError::Maintenance(error.to_string()))?
            .forget();
        if let Some(fence) = self.refusal.lock().unwrap().clone() {
            return Err(OplogError::Fenced(fence));
        }
        self.entries.lock().unwrap().extend(chunk.iter().cloned());
        Ok(chunk.len() as u64)
    }

    async fn verify_persisted(
        &self,
        _entries: &[(OplogIndex, OplogEntry)],
    ) -> OplogArchiveResult<()> {
        Ok(())
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .last_key_value()
            .map(|(idx, _)| *idx)
            .unwrap_or(OplogIndex::NONE))
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError> {
        let mut entries = self.entries.lock().unwrap();
        let old = entries.len();
        entries.retain(|idx, _| *idx > last_dropped_id);
        Ok((old - entries.len()) as u64)
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        Ok(self.entries.lock().unwrap().len() as u64)
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.current_oplog_index().await
    }
}

struct SingletonArchiveService(Arc<GatedArchive>);

impl Debug for SingletonArchiveService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingletonArchiveService").finish()
    }
}

#[async_trait]
impl OplogArchiveService for SingletonArchiveService {
    async fn open(
        &self,
        _: &OwnedAgentId,
        _: AgentMode,
        _: Option<ShardEpoch>,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        self.0.clone()
    }
    async fn open_fresh(
        &self,
        _: &OwnedAgentId,
        _: AgentMode,
        _: Option<ShardEpoch>,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        self.0.clone()
    }
    async fn delete(&self, _: &OwnedAgentId, _: AgentMode) -> OplogArchiveResult<()> {
        Ok(())
    }
    async fn read_source(
        &self,
        _: &OwnedAgentId,
        _: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.0.read_source(idx, n).await.unwrap()
    }
    async fn exists(&self, _: &OwnedAgentId, _: AgentMode) -> bool {
        self.0.length().await.unwrap() != 0
    }
    async fn scan_for_component(
        &self,
        _: &EnvironmentId,
        _: &ComponentId,
        _: Option<AgentMode>,
        cursor: ScanCursor,
        _: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        Ok((cursor, Vec::new()))
    }
    async fn get_last_index(&self, _: &OwnedAgentId, _: AgentMode) -> OplogIndex {
        self.0.get_last_index().await.unwrap()
    }
}

pub struct Fixture {
    pub oplog: Arc<EphemeralOplog>,
    pub archive: Arc<GatedArchive>,
}

pub async fn fixture(threshold: u64) -> Fixture {
    let archive = Arc::new(GatedArchive::default());
    let primary: Arc<dyn OplogService> = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            100,
            threshold,
            1024,
            RetryConfig::default(),
        )
        .await,
    );
    let archive_service: Arc<dyn OplogArchiveService> =
        Arc::new(SingletonArchiveService(archive.clone()));
    let service =
        MultiLayerOplogService::new(primary.clone(), nev![archive_service], 100, threshold);
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "ephemeral-actor-test".into(),
        },
    );
    let lower: Arc<dyn OplogArchive + Send + Sync> = archive.clone();
    let lower = nev![lower];
    let (transfer_tx, transfer_rx) = tokio::sync::mpsc::unbounded_channel();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let transfer_fiber = new_transfer_fiber();
    let transfer = EphemeralOplog::spawn_background_transfer(
        owned_agent_id.clone(),
        lower.clone(),
        service.clone(),
        transfer_rx,
        start_rx,
    );
    MultiLayerOplogService::start_transfer(&transfer_fiber, start_tx, transfer).await;
    let oplog = Arc::new(
        EphemeralOplog::new(
            owned_agent_id,
            AgentMode::Ephemeral,
            AgentFingerprint::new(),
            OplogIndex::NONE,
            threshold,
            primary,
            lower,
            transfer_tx,
            transfer_fiber,
            service,
            Box::new(|| {}),
        )
        .await,
    );
    Fixture { oplog, archive }
}

fn entry(n: usize) -> OplogEntry {
    if n.is_multiple_of(2) {
        OplogEntry::suspend().rounded()
    } else {
        OplogEntry::restart().rounded()
    }
}

#[test]
#[timeout("30s")]
async fn threshold_flush_does_not_block_add_and_deferred_returns_receipt() {
    let fixture = fixture(1).await;
    assert_eq!(
        fixture.oplog.add(entry(0)).await.unwrap(),
        OplogIndex::INITIAL
    );
    assert_eq!(
        fixture.oplog.add(entry(1)).await.unwrap(),
        OplogIndex::from_u64(2)
    );
    fixture.archive.wait_for_appends(1).await;

    let receipt = fixture.oplog.commit(CommitLevel::Deferred).await.unwrap();
    assert_eq!(
        receipt.keys().copied().collect::<Vec<_>>(),
        vec![OplogIndex::INITIAL, OplogIndex::from_u64(2)]
    );
    fixture.archive.release(1);
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn always_commit_waits_for_prior_write_even_with_empty_residual_batch() {
    let fixture = fixture(0).await;
    fixture.oplog.add(entry(0)).await.unwrap();
    fixture.archive.wait_for_appends(1).await;
    let commit = fixture.oplog.commit(CommitLevel::Always);
    tokio::pin!(commit);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut commit)
            .await
            .is_err()
    );
    fixture.archive.release(1);
    let receipt = commit.await.unwrap();
    assert_eq!(receipt.len(), 1);
    assert_eq!(fixture.archive.append_calls.load(Ordering::Relaxed), 1);
}

#[test]
#[timeout("30s")]
async fn read_exact_combines_persisted_handed_off_and_buffer_without_reading_buffered_suffix() {
    let fixture = fixture(1).await;
    let expected: Vec<_> = (0..5).map(entry).collect();
    fixture.archive.release(1);
    for entry in &expected[..2] {
        fixture.oplog.add(entry.clone()).await.unwrap();
    }
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    for entry in &expected[2..] {
        fixture.oplog.add(entry.clone()).await.unwrap();
    }
    fixture.archive.wait_for_appends(2).await;

    let all = fixture.oplog.read_exact(OplogIndex::INITIAL, 5).await;
    assert_eq!(all.into_values().collect::<Vec<_>>(), expected);
    let reads = fixture.archive.read_calls.load(Ordering::Relaxed);
    let suffix = fixture.oplog.read_exact(OplogIndex::from_u64(3), 3).await;
    assert_eq!(suffix.into_values().collect::<Vec<_>>(), expected[2..]);
    assert_eq!(fixture.archive.read_calls.load(Ordering::Relaxed), reads);

    let session_key = golem_common::model::durable_stream::StreamInvocationId {
        callee_environment_id: fixture.oplog.owned_agent_id.environment_id,
        callee: fixture.oplog.owned_agent_id.agent_id.clone(),
        callee_fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        idempotency_key: golem_common::model::IdempotencyKey::fresh(),
    };
    let snapshot = fixture
        .oplog
        .run_job(|done| EphemeralJob::RawDurableStreamSessionStatus { session_key, done })
        .await;
    assert_eq!(snapshot.committed, OplogIndex::from_u64(2));
    assert_eq!(snapshot.watermark, OplogIndex::from_u64(5));
    assert_eq!(
        snapshot.buffer.into_iter().collect::<Vec<_>>(),
        expected[2..]
    );
    fixture.archive.release(2);
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
}

#[test]
#[timeout("30s")]
async fn bounded_writer_queue_backpressures_fourth_threshold_flush_and_close_drains() {
    let fixture = fixture(0).await;
    for n in 0..3 {
        fixture.oplog.add(entry(n)).await.unwrap();
    }
    fixture.archive.wait_for_appends(1).await;

    let fourth = fixture.oplog.add(entry(3));
    tokio::pin!(fourth);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut fourth)
            .await
            .is_err()
    );
    fixture.archive.release(1);
    assert_eq!(fourth.await.unwrap(), OplogIndex::from_u64(4));

    let closed = fixture.oplog.closed();
    fixture.oplog.retire();
    fixture.archive.release(3);
    assert_eq!(closed.await, Ok(()));
    assert_eq!(fixture.archive.length().await.unwrap(), 4);
}

#[test]
#[timeout("30s")]
async fn shutdown_waits_for_handed_off_writes_without_flushing_buffered_tail() {
    let fixture = fixture(1).await;
    fixture.oplog.add(entry(0)).await.unwrap();
    fixture.oplog.add(entry(1)).await.unwrap();
    fixture.archive.wait_for_appends(1).await;
    fixture.oplog.add(entry(2)).await.unwrap();
    let shutdown_handle = fixture.oplog.executor_shutdown_handle();

    shutdown_handle.fence();
    let shutdown = tokio::spawn(async move { shutdown_handle.close_and_wait().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), fixture.oplog.closed())
            .await
            .is_err()
    );

    fixture.archive.release(1);
    shutdown.await.unwrap().unwrap();
    assert_eq!(fixture.archive.length().await.unwrap(), 2);
    assert_eq!(fixture.archive.append_calls.load(Ordering::Relaxed), 1);
}

#[test]
#[timeout("30s")]
async fn executor_shutdown_marks_last_handle_drop_as_non_flushing() {
    let fixture = fixture(10).await;
    fixture.oplog.add(entry(0)).await.unwrap();
    let archive = fixture.archive.clone();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let tasks = crate::services::active_agents::InvocationLoops::new(shutdown.clone());
    let registered: Arc<dyn Oplog> = fixture.oplog.clone();
    tasks.register_owner_oplog(registered);
    let task = tasks.spawn_entity({
        let oplog = fixture.oplog.clone();
        async move {
            let _oplog = oplog;
            std::future::pending::<()>().await;
        }
    });
    drop(fixture.oplog);

    shutdown.cancel();
    tasks.wait_for_exit().await.unwrap();
    assert_eq!(task.await.unwrap(), None);
    assert_eq!(archive.length().await.unwrap(), 0);
    assert_eq!(archive.append_calls.load(Ordering::Relaxed), 0);
}

#[test]
#[timeout("30s")]
async fn saved_executor_shutdown_fence_preserves_admitted_writer_append() {
    let fixture = fixture(0).await;
    fixture.oplog.add(entry(0)).await.unwrap();
    fixture.archive.wait_for_appends(1).await;
    let shutdown = fixture.oplog.executor_shutdown_handle();
    let weak = Arc::downgrade(&fixture.oplog);
    let archive = fixture.archive.clone();

    drop(fixture.oplog);
    assert!(weak.upgrade().is_none());
    shutdown.fence();
    let mut shutdown_close = Box::pin(shutdown.close_and_wait());
    assert!(futures::poll!(shutdown_close.as_mut()).is_pending());

    archive.release(1);
    shutdown_close.await.unwrap();
    assert_eq!(archive.length().await.unwrap(), 1);
}

#[test]
#[timeout("30s")]
async fn stale_executor_shutdown_fence_does_not_stop_newer_transfer() {
    let fixture = fixture(10).await;
    let shutdown = fixture.oplog.executor_shutdown_handle();
    let service = fixture.oplog.multi_layer_oplog_service.clone();
    let agent_id = fixture.oplog.owned_agent_id.agent_id.clone();
    let old = fixture.oplog.transfer_fiber.clone();
    let newer = new_transfer_fiber();
    service.register_transfer(agent_id.clone(), &newer);
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let transfer = tokio::spawn(async move {
        if start_rx.await.is_ok() {
            let _ = release_rx.await;
        }
    });
    MultiLayerOplogService::start_transfer(&newer, start_tx, transfer).await;

    shutdown.fence();
    shutdown.fence();
    shutdown.close_and_wait().await.unwrap();

    assert!(service.has_registered_transfer(&agent_id, &newer));
    MultiLayerOplogService::transfer_closed(&old).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            MultiLayerOplogService::transfer_closed(&newer),
        )
        .await
        .is_err()
    );
    release_tx.send(()).unwrap();
    MultiLayerOplogService::transfer_closed(&newer)
        .await
        .unwrap();
}

#[test]
#[timeout("30s")]
async fn receipt_overflow_retains_a_detectable_gap_and_storage_barrier_covers_it() {
    let fixture = fixture(0).await;
    let count = MAX_RETAINED_RECEIPT_BATCHES + 7;
    fixture.archive.release(count);
    let expected: Vec<_> = (0..count).map(entry).collect();
    for entry in &expected {
        fixture.oplog.add(entry.clone()).await.unwrap();
    }
    let receipts = fixture.oplog.commit(CommitLevel::Deferred).await.unwrap();
    assert_eq!(receipts.len(), MAX_RETAINED_RECEIPT_BATCHES);
    assert_eq!(*receipts.first_key_value().unwrap().0, OplogIndex::INITIAL);
    assert_eq!(receipts.last_key_value().unwrap().0.as_u64(), count as u64);
    assert!(!receipts.contains_key(&OplogIndex::from_u64(2)));
    assert_eq!(fixture.archive.read_calls.load(Ordering::Relaxed), 0);

    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(
        fixture
            .archive
            .read_source(OplogIndex::INITIAL, count as u64)
            .await
            .unwrap()
            .into_values()
            .collect::<Vec<_>>(),
        expected
    );
    fixture.oplog.retire();
    fixture.oplog.closed().await.unwrap();
}

#[test]
#[timeout("30s")]
async fn failed_writer_is_joined_and_reported_by_close() {
    let fixture = fixture(0).await;
    fixture.oplog.add(entry(0)).await.unwrap();
    fixture.archive.wait_for_appends(1).await;
    fixture.archive.append_permits.close();
    fixture.oplog.retire();
    assert!(fixture.oplog.closed().await.is_err());
    assert_eq!(fixture.archive.length().await.unwrap(), 0);
}

fn refusal(fixture: &Fixture) -> OplogFence {
    OplogFence {
        agent_id: fixture.oplog.owned_agent_id.agent_id.clone(),
        expected_epoch: ShardEpoch(5),
        actual_epoch: Some(ShardEpoch(6)),
    }
}

#[test]
#[timeout("30s")]
async fn a_refused_batch_fails_the_commit_waiting_on_it_and_every_later_write() {
    let fixture = fixture(100).await;
    *fixture.archive.refusal.lock().unwrap() = Some(refusal(&fixture));
    fixture.archive.release(1);
    let refused = entry(0);
    fixture.oplog.add(refused.clone()).await.unwrap();

    match fixture.oplog.commit(CommitLevel::Always).await {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.expected_epoch, ShardEpoch(5));
            assert_eq!(fence.actual_epoch, Some(ShardEpoch(6)));
        }
        other => panic!("expected the commit to be fenced, got {other:?}"),
    }
    assert!(fixture.oplog.fence().is_some());

    // Refused on the handle, without asking the archive again.
    assert!(matches!(
        fixture.oplog.add(entry(1)).await,
        Err(OplogError::Fenced(_))
    ));
    assert!(matches!(
        fixture.oplog.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    assert_eq!(fixture.archive.append_calls.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.archive.length().await, Ok(0));

    // The refused entry was never committed, and this handle still reads it.
    assert_eq!(
        fixture
            .oplog
            .read_exact(OplogIndex::INITIAL, 1)
            .await
            .into_values()
            .collect::<Vec<_>>(),
        vec![refused]
    );
}

#[test]
#[timeout("30s")]
async fn a_deferred_batch_refused_after_its_commit_returned_fails_the_next_write() {
    let fixture = fixture(100).await;
    *fixture.archive.refusal.lock().unwrap() = Some(refusal(&fixture));
    fixture.oplog.add(entry(0)).await.unwrap();
    fixture.oplog.commit(CommitLevel::Deferred).await.unwrap();
    fixture.archive.wait_for_appends(1).await;
    fixture.archive.release(1);

    // The storage barrier queues behind the refused batch, so this commit reads the fence the
    // writer latched rather than reporting the batch as stored.
    assert!(matches!(
        fixture.oplog.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    assert!(matches!(
        fixture.oplog.add(entry(1)).await,
        Err(OplogError::Fenced(_))
    ));
    assert_eq!(fixture.archive.append_calls.load(Ordering::Relaxed), 1);
}
