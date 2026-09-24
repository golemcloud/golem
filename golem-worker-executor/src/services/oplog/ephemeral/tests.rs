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
    MultiLayerOplogService, OplogArchiveService, new_transfer_fiber,
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

struct GatedArchive {
    entries: Mutex<BTreeMap<OplogIndex, OplogEntry>>,
    append_started: Notify,
    append_permits: Semaphore,
    append_calls: AtomicUsize,
    read_calls: AtomicUsize,
}

impl Default for GatedArchive {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            append_started: Notify::new(),
            append_permits: Semaphore::new(0),
            append_calls: AtomicUsize::new(0),
            read_calls: AtomicUsize::new(0),
        }
    }
}

impl Debug for GatedArchive {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatedArchive").finish()
    }
}

impl GatedArchive {
    async fn wait_for_appends(&self, count: usize) {
        while self.append_calls.load(Ordering::Acquire) < count {
            self.append_started.notified().await;
        }
    }

    fn release(&self, count: usize) {
        self.append_permits.add_permits(count);
    }
}

#[async_trait]
impl OplogArchive for GatedArchive {
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_calls.fetch_add(1, Ordering::Relaxed);
        let end = idx.as_u64().saturating_add(n);
        self.entries
            .lock()
            .unwrap()
            .range(idx..OplogIndex::from_u64(end))
            .map(|(idx, entry)| (*idx, entry.clone()))
            .collect()
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        self.append_calls.fetch_add(1, Ordering::Release);
        self.append_started.notify_one();
        self.append_permits.acquire().await.unwrap().forget();
        self.entries.lock().unwrap().extend(chunk.iter().cloned());
        chunk.len() as u64
    }

    async fn verify_persisted(&self, _entries: &[(OplogIndex, OplogEntry)]) {}

    async fn current_oplog_index(&self) -> OplogIndex {
        self.entries
            .lock()
            .unwrap()
            .last_key_value()
            .map(|(idx, _)| *idx)
            .unwrap_or(OplogIndex::NONE)
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let mut entries = self.entries.lock().unwrap();
        let old = entries.len();
        entries.retain(|idx, _| *idx > last_dropped_id);
        (old - entries.len()) as u64
    }

    async fn length(&self) -> u64 {
        self.entries.lock().unwrap().len() as u64
    }

    async fn get_last_index(&self) -> OplogIndex {
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
    async fn open(&self, _: &OwnedAgentId, _: AgentMode) -> Arc<dyn OplogArchive + Send + Sync> {
        self.0.clone()
    }
    async fn open_fresh(
        &self,
        _: &OwnedAgentId,
        _: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        self.0.clone()
    }
    async fn delete(&self, _: &OwnedAgentId, _: AgentMode) {}
    async fn read_source(
        &self,
        _: &OwnedAgentId,
        _: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.0.read_source(idx, n).await
    }
    async fn exists(&self, _: &OwnedAgentId, _: AgentMode) -> bool {
        self.0.length().await != 0
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
        self.0.get_last_index().await
    }
}

struct Fixture {
    oplog: Arc<EphemeralOplog>,
    archive: Arc<GatedArchive>,
}

async fn fixture(threshold: u64) -> Fixture {
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
    assert_eq!(fixture.oplog.add(entry(0)).await, OplogIndex::INITIAL);
    assert_eq!(fixture.oplog.add(entry(1)).await, OplogIndex::from_u64(2));
    fixture.archive.wait_for_appends(1).await;

    let receipt = fixture.oplog.commit(CommitLevel::Deferred).await;
    assert_eq!(
        receipt.keys().copied().collect::<Vec<_>>(),
        vec![OplogIndex::INITIAL, OplogIndex::from_u64(2)]
    );
    fixture.archive.release(1);
    fixture.oplog.commit(CommitLevel::Always).await;
}

#[test]
#[timeout("30s")]
async fn always_commit_waits_for_prior_write_even_with_empty_residual_batch() {
    let fixture = fixture(0).await;
    fixture.oplog.add(entry(0)).await;
    fixture.archive.wait_for_appends(1).await;
    let commit = fixture.oplog.commit(CommitLevel::Always);
    tokio::pin!(commit);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut commit)
            .await
            .is_err()
    );
    fixture.archive.release(1);
    let receipt = commit.await;
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
        fixture.oplog.add(entry.clone()).await;
    }
    fixture.oplog.commit(CommitLevel::Always).await;
    for entry in &expected[2..] {
        fixture.oplog.add(entry.clone()).await;
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
    fixture.oplog.commit(CommitLevel::Always).await;
}

#[test]
#[timeout("30s")]
async fn bounded_writer_queue_backpressures_fourth_threshold_flush_and_close_drains() {
    let fixture = fixture(0).await;
    for n in 0..3 {
        fixture.oplog.add(entry(n)).await;
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
    assert_eq!(fourth.await, OplogIndex::from_u64(4));

    let closed = fixture.oplog.closed();
    fixture.oplog.retire();
    fixture.archive.release(3);
    assert_eq!(closed.await, Ok(()));
    assert_eq!(fixture.archive.length().await, 4);
}

#[test]
#[timeout("30s")]
async fn receipt_overflow_retains_a_detectable_gap_and_storage_barrier_covers_it() {
    let fixture = fixture(0).await;
    let count = MAX_RETAINED_RECEIPT_BATCHES + 7;
    fixture.archive.release(count);
    let expected: Vec<_> = (0..count).map(entry).collect();
    for entry in &expected {
        fixture.oplog.add(entry.clone()).await;
    }
    let receipts = fixture.oplog.commit(CommitLevel::Deferred).await;
    assert_eq!(receipts.len(), MAX_RETAINED_RECEIPT_BATCHES);
    assert_eq!(*receipts.first_key_value().unwrap().0, OplogIndex::INITIAL);
    assert_eq!(receipts.last_key_value().unwrap().0.as_u64(), count as u64);
    assert!(!receipts.contains_key(&OplogIndex::from_u64(2)));
    assert_eq!(fixture.archive.read_calls.load(Ordering::Relaxed), 0);

    fixture.oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        fixture
            .archive
            .read_source(OplogIndex::INITIAL, count as u64)
            .await
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
    fixture.oplog.add(entry(0)).await;
    fixture.archive.wait_for_appends(1).await;
    fixture.archive.append_permits.close();
    fixture.oplog.retire();
    assert!(fixture.oplog.closed().await.is_err());
    assert_eq!(fixture.archive.length().await, 0);
}
