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
use crate::services::oplog::compressed::CompressedOplogArchiveService;
use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveService};
use crate::storage::indexed::IndexedStorage;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use assert2::check;
use golem_common::config::RedisConfig;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, Principal};
use golem_common::model::component::ComponentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{AgentError, LogLevel};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AgentFingerprint, AgentMetadata, AgentStatusRecord, IdempotencyKey, OwnedAgentId,
};
use golem_common::model::{AgentInvocationPayload, RetryConfig};
use golem_common::redis::RedisPool;
use golem_common::schema::{BinaryValuePayload, FromSchema, IntoTypedSchemaValue, SchemaValue};
use golem_common::tracing::{TracingConfig, init_tracing};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use nonempty_collections::nev;
use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use test_r::{test, test_dep};
use tokio::sync::{Mutex, Notify, oneshot};
use tracing::{debug, info};
use uuid::Uuid;

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing(&TracingConfig::test("op-log-tests"), |_output| {
            golem_common::tracing::filter::boxed::debug_env_with_directives(Vec::new())
        });
        Self
    }
}

#[test_dep(scope = PerWorker)]
fn tracing() -> Tracing {
    Tracing::init()
}

#[derive(Debug, Default)]
struct ArchiveCallCounts {
    open: AtomicUsize,
    open_fresh: AtomicUsize,
    read: AtomicUsize,
    exists: AtomicUsize,
    get_last_index: AtomicUsize,
    archive_read: AtomicUsize,
    append: AtomicUsize,
    current_index: AtomicUsize,
    length: AtomicUsize,
    archive_last_index: AtomicUsize,
}

#[derive(Debug)]
struct RecordingArchiveService {
    inner: Arc<dyn OplogArchiveService>,
    calls: Arc<ArchiveCallCounts>,
}

#[derive(Debug)]
struct RecordingArchive {
    inner: Arc<dyn OplogArchive + Send + Sync>,
    calls: Arc<ArchiveCallCounts>,
}

#[async_trait::async_trait]
impl OplogArchiveService for RecordingArchiveService {
    async fn open(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        self.calls.open.fetch_add(1, Ordering::Relaxed);
        Arc::new(RecordingArchive {
            inner: self.inner.open(id, mode).await,
            calls: self.calls.clone(),
        })
    }
    async fn open_fresh(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        self.calls.open_fresh.fetch_add(1, Ordering::Relaxed);
        Arc::new(RecordingArchive {
            inner: self.inner.open_fresh(id, mode).await,
            calls: self.calls.clone(),
        })
    }
    async fn delete(&self, id: &OwnedAgentId, mode: AgentMode) {
        self.inner.delete(id, mode).await
    }
    async fn read(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        self.calls.read.fetch_add(1, Ordering::Relaxed);
        self.inner.read(id, mode, idx, n).await
    }
    async fn exists(&self, id: &OwnedAgentId, mode: AgentMode) -> bool {
        self.calls.exists.fetch_add(1, Ordering::Relaxed);
        self.inner.exists(id, mode).await
    }
    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, modes, cursor, count)
            .await
    }
    async fn get_last_index(&self, id: &OwnedAgentId, mode: AgentMode) -> OplogIndex {
        self.calls.get_last_index.fetch_add(1, Ordering::Relaxed);
        self.inner.get_last_index(id, mode).await
    }
}

#[async_trait::async_trait]
impl OplogArchive for RecordingArchive {
    async fn read(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        self.calls.archive_read.fetch_add(1, Ordering::Relaxed);
        self.inner.read(idx, n).await
    }
    async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>) -> u64 {
        self.calls.append.fetch_add(1, Ordering::Relaxed);
        self.inner.append(chunk).await
    }
    async fn current_oplog_index(&self) -> OplogIndex {
        self.calls.current_index.fetch_add(1, Ordering::Relaxed);
        self.inner.current_oplog_index().await
    }
    async fn drop_prefix(&self, idx: OplogIndex) -> u64 {
        self.inner.drop_prefix(idx).await
    }
    async fn length(&self) -> u64 {
        self.calls.length.fetch_add(1, Ordering::Relaxed);
        self.inner.length().await
    }
    async fn get_last_index(&self) -> OplogIndex {
        self.calls
            .archive_last_index
            .fetch_add(1, Ordering::Relaxed);
        self.inner.get_last_index().await
    }
}

fn make_agent_metadata(
    agent_id: AgentId,
    created_by: AccountId,
    environment_id: EnvironmentId,
) -> AgentMetadata {
    AgentMetadata {
        agent_id,
        env: vec![],
        environment_id,
        created_by,
        created_by_email: AccountEmail::new("test@golem"),
        config: Vec::new(),
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord::default(),
        original_phantom_id: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
    }
}

fn default_last_known_status() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
        AgentStatusRecord::default(),
    )))
}

fn default_execution_status(
    agent_mode: AgentMode,
) -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        agent_mode,
        timestamp: Timestamp::now_utc(),
    })))
}

struct BlockingArchiveService {
    inner: Arc<dyn OplogArchiveService>,
    append_started: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release_append: Arc<Notify>,
    append_finished: Arc<Notify>,
}

impl Debug for BlockingArchiveService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockingArchiveService").finish()
    }
}

#[async_trait]
impl OplogArchiveService for BlockingArchiveService {
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(BlockingArchive {
            inner: self.inner.open(owned_agent_id, agent_mode).await,
            append_started: self.append_started.clone(),
            release_append: self.release_append.clone(),
            append_finished: self.append_finished.clone(),
        })
    }

    async fn open_fresh(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(BlockingArchive {
            inner: self.inner.open_fresh(owned_agent_id, agent_mode).await,
            append_started: self.append_started.clone(),
            release_append: self.release_append.clone(),
            append_finished: self.append_finished.clone(),
        })
    }

    async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
        self.inner.delete(owned_agent_id, agent_mode).await
    }

    async fn read(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read(owned_agent_id, agent_mode, idx, n).await
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
        self.inner.exists(owned_agent_id, agent_mode).await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, modes, cursor, count)
            .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
        self.inner.get_last_index(owned_agent_id, agent_mode).await
    }
}

struct BlockingArchive {
    inner: Arc<dyn OplogArchive + Send + Sync>,
    append_started: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release_append: Arc<Notify>,
    append_finished: Arc<Notify>,
}

impl Debug for BlockingArchive {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockingArchive").finish()
    }
}

#[async_trait]
impl OplogArchive for BlockingArchive {
    async fn read(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read(idx, n).await
    }

    async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>) -> u64 {
        if let Some(sender) = self.append_started.lock().await.take() {
            let _ = sender.send(());
        }
        self.release_append.notified().await;
        let result = self.inner.append(chunk).await;
        self.append_finished.notify_one();
        result
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.inner.drop_prefix(last_dropped_id).await
    }

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.inner.get_last_index().await
    }
}

#[test]
async fn ephemeral_create_baseline_uses_lower_storage_and_checked_reads_find_it(
    _tracing: &Tracing,
) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage,
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let calls = Arc::new(ArchiveCallCounts::default());
    let lower1: Arc<dyn OplogArchiveService> = Arc::new(RecordingArchiveService {
        inner: Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        )),
        calls: calls.clone(),
    });
    let lower2: Arc<dyn OplogArchiveService> = Arc::new(RecordingArchiveService {
        inner: Arc::new(CompressedOplogArchiveService::new(
            indexed_storage,
            2,
            RetryConfig::default(),
        )),
        calls: calls.clone(),
    });
    let service = MultiLayerOplogService::new(primary.clone(), nev![lower1, lower2], 10, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "ephemeral-baseline".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = OplogEntry::create(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        Vec::new(),
        environment_id,
        account_id,
        None,
        100,
        100,
        HashSet::new(),
        Vec::new(),
        None,
        Uuid::new_v4(),
    )
    .rounded();

    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = service
        .create(
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    assert!(!primary.exists(&owned_agent_id, AgentMode::Ephemeral).await);
    assert_eq!(calls.exists.load(Ordering::Relaxed), 0);
    assert_eq!(calls.get_last_index.load(Ordering::Relaxed), 0);
    assert_eq!(calls.read.load(Ordering::Relaxed), 0);
    assert_eq!(calls.archive_read.load(Ordering::Relaxed), 0);
    assert!(calls.open.load(Ordering::Relaxed) >= 1);
    assert!(calls.append.load(Ordering::Relaxed) >= 1);
    assert!(calls.length.load(Ordering::Relaxed) >= 1);

    assert!(service.exists(&owned_agent_id, AgentMode::Ephemeral).await);
    assert!(calls.exists.load(Ordering::Relaxed) >= 1);
    let entries = service
        .read(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));
    assert!(calls.read.load(Ordering::Relaxed) >= 1);

    drop(oplog);
}

#[test]
async fn fresh_ephemeral_create_does_not_probe_lower_storage(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage,
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let calls = Arc::new(ArchiveCallCounts::default());
    let lower1: Arc<dyn OplogArchiveService> = Arc::new(RecordingArchiveService {
        inner: Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        )),
        calls: calls.clone(),
    });
    let lower2: Arc<dyn OplogArchiveService> = Arc::new(RecordingArchiveService {
        inner: Arc::new(CompressedOplogArchiveService::new(
            indexed_storage,
            2,
            RetryConfig::default(),
        )),
        calls: calls.clone(),
    });
    let service = MultiLayerOplogService::new(primary.clone(), nev![lower1, lower2], 10, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fresh-ephemeral".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = OplogEntry::create(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        Vec::new(),
        environment_id,
        account_id,
        None,
        100,
        100,
        HashSet::new(),
        Vec::new(),
        None,
        Uuid::new_v4(),
    )
    .rounded();

    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = service
        .create_fresh(
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    assert!(!primary.exists(&owned_agent_id, AgentMode::Ephemeral).await);
    assert_eq!(calls.open.load(Ordering::Relaxed), 0);
    assert_eq!(calls.open_fresh.load(Ordering::Relaxed), 2);
    assert_eq!(calls.exists.load(Ordering::Relaxed), 0);
    assert_eq!(calls.get_last_index.load(Ordering::Relaxed), 0);
    assert_eq!(calls.read.load(Ordering::Relaxed), 0);
    assert_eq!(calls.archive_read.load(Ordering::Relaxed), 0);
    assert_eq!(calls.length.load(Ordering::Relaxed), 0);
    assert_eq!(calls.archive_last_index.load(Ordering::Relaxed), 0);
    assert_eq!(calls.append.load(Ordering::Relaxed), 1);

    assert!(service.exists(&owned_agent_id, AgentMode::Ephemeral).await);
    let entries = service
        .read(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));

    drop(oplog);
}

#[test]
async fn open_add_and_read_back(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();

    let last_oplog_idx = oplog.current_oplog_index().await;
    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);

    let entries = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            last_oplog_idx.next(),
            3,
        )
        .await;
    assert_eq!(
        entries.into_values().collect::<Vec<_>>(),
        vec![entry1, entry2, entry3]
    );
}

#[test]
async fn open_add_and_read_back_many(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage.clone(),
        blob_storage,
        100,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();
    let entry4 = OplogEntry::interrupted().rounded();
    let entry5 = OplogEntry::no_op().rounded();

    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;
    oplog.add(entry4.clone()).await;
    oplog.add(entry5.clone()).await; // uncommitted entries

    let read_count = indexed_storage.read_count();
    let buffered_entries = oplog
        .read_many(OplogIndex::from_u64(4), 2)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(buffered_entries, vec![entry4.clone(), entry5.clone()]);
    assert_eq!(indexed_storage.read_count(), read_count);

    assert_eq!(oplog.read(OplogIndex::from_u64(4)).await, entry4.clone());
    assert_eq!(oplog.read(OplogIndex::from_u64(5)).await, entry5.clone());
    assert_eq!(indexed_storage.read_count(), read_count);

    let entries = oplog
        .read_many(OplogIndex::INITIAL, 5)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(
        entries,
        vec![entry1, entry2, entry3, entry4, entry5.clone()]
    );
    assert_eq!(indexed_storage.read_count(), read_count + 1);

    let entry = oplog
        .read_many(OplogIndex::from_u64(5), 1)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entry, vec![entry5]);
    assert_eq!(indexed_storage.read_count(), read_count + 1);

    let read_count = indexed_storage.read_count();
    assert!(oplog.read_many(OplogIndex::from_u64(5), 0).await.is_empty());
    assert_eq!(indexed_storage.read_count(), read_count);
}

#[test]
async fn open_add_and_read_back_ephemeral(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();

    let last_oplog_idx = oplog.current_oplog_index().await;
    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);

    let entries = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            last_oplog_idx.next(),
            3,
        )
        .await;
    assert_eq!(
        entries.into_values().collect::<Vec<_>>(),
        vec![entry1, entry2, entry3]
    );
}

#[test]
async fn open_add_and_read_back_many_ephemeral(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();
    let entry4 = OplogEntry::interrupted().rounded();

    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;
    oplog.add(entry4.clone()).await; // uncommitted

    let entries = oplog
        .read_many(OplogIndex::INITIAL, 4)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2, entry3, entry4]);
}

#[test]
async fn ephemeral_read_many_committed_only(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let entry1 = OplogEntry::suspend().rounded();
    let entry2 = OplogEntry::exited().rounded();
    let entry3 = OplogEntry::interrupted().rounded();

    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;

    // All committed, no buffer entries
    let entries = oplog
        .read_many(OplogIndex::INITIAL, 3)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2, entry3]);
}

#[test]
async fn ephemeral_read_many_uncommitted_only(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        1000, // high limit so nothing auto-commits
        1000,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let entry1 = OplogEntry::suspend().rounded();
    let entry2 = OplogEntry::exited().rounded();

    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    // No commit — entries only in the buffer

    let entries = oplog
        .read_many(OplogIndex::INITIAL, 2)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2]);
}

#[test]
async fn ephemeral_read_many_partial_range(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        1000, // high ephemeral limit so nothing auto-commits from buffer
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries = Vec::new();
    for i in 0..10 {
        let entry = OplogEntry::Error {
            timestamp,
            error: AgentError::Unknown(i.to_string()),
            retry_from: OplogIndex::NONE,
            inside_atomic_region: false,
            retry_policy_state: None,
        }
        .rounded();
        oplog.add(entry.clone()).await;
        entries.push(entry);
    }
    oplog.commit(CommitLevel::Always).await;

    // Add 2 more uncommitted
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await;
    oplog.add(uncommitted2.clone()).await;
    entries.push(uncommitted1);
    entries.push(uncommitted2);

    // Read a sub-range from the middle spanning committed and uncommitted
    let mid_entries = oplog
        .read_many(OplogIndex::from_u64(8), 4)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(mid_entries, entries[7..11].to_vec());

    // Read just the first 3
    let first3 = oplog
        .read_many(OplogIndex::INITIAL, 3)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(first3, entries[0..3].to_vec());

    // Read the last 2 (uncommitted only)
    let last2 = oplog
        .read_many(OplogIndex::from_u64(11), 2)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(last2, entries[10..12].to_vec());

    // Read all
    let all = oplog
        .read_many(OplogIndex::INITIAL, 12)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(all, entries);
}

#[test]
async fn ephemeral_read_many_across_archive_layers(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: AgentError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect();

    let initial_oplog_idx = oplog.current_oplog_index().await;

    for entry in &entries {
        oplog.add(entry.clone()).await;
    }
    oplog.commit(CommitLevel::Always).await;

    // Add 2 uncommitted entries
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await;
    oplog.add(uncommitted2.clone()).await;
    entries.push(uncommitted1);
    entries.push(uncommitted2);

    // Wait for background archiving to move entries between layers
    tokio::time::sleep(Duration::from_secs(2)).await;

    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    // Read first 10 — should come from lower layers
    let first10 = oplog
        .read_many(initial_oplog_idx.next(), 10)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(first10, entries[..10].to_vec());

    // Read last 10 — includes uncommitted entries from buffer
    let last10 = oplog
        .read_many(oplog.current_oplog_index().await.subtract(10).next(), 10)
        .await
        .into_values()
        .collect::<Vec<_>>();
    let original_last10 = entries
        .iter()
        .rev()
        .take(10)
        .rev()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(last10, original_last10);

    // Read all entries
    let all = oplog
        .read_many(initial_oplog_idx.next(), entries.len() as u64)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(all, entries);
}

#[test]
async fn ephemeral_read_many_zero_returns_empty(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    oplog.add(OplogEntry::suspend().rounded()).await;

    let entries = oplog.read_many(OplogIndex::INITIAL, 0).await;
    assert!(entries.is_empty());
}

#[test]
async fn entries_with_small_payload(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let last_oplog_idx = oplog.current_oplog_index().await;
    let (start_idx, end_idx) = oplog
        .add_completed_host_call(
            HostFunctionName::Custom("f1".to_string()),
            &HostRequest::Custom("request".to_string().into_typed_schema_value().unwrap()),
            &HostResponse::Custom("response".to_string().into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadRemote,
            None,
        )
        .await
        .unwrap();
    let entry_start = oplog.read(start_idx).await.rounded();
    let entry_end = oplog.read(end_idx).await.rounded();
    let entry2 = oplog
        .add_agent_invocation_started(AgentInvocation::AgentMethod {
            idempotency_key: IdempotencyKey::fresh(),
            method_name: "f2".to_string(),
            input: SchemaValue::Record {
                fields: vec![SchemaValue::String("request".to_string())],
            },
            invocation_context: InvocationContextStack::fresh_rounded(),
            principal: Principal::anonymous(),
        })
        .await
        .unwrap()
        .rounded();
    let entry3 = oplog
        .add_agent_invocation_finished(
            &AgentInvocationResult::AgentMethod {
                output: SchemaValue::Record {
                    fields: vec![SchemaValue::String("response".to_string())],
                },
            },
            Some("f2".to_string()),
            42,
            ComponentRevision::INITIAL,
        )
        .await
        .unwrap()
        .rounded();

    let desc = oplog
        .create_snapshot_based_update_description(
            ComponentRevision::new(11).unwrap(),
            vec![1, 2, 3],
            "application/octet-stream".to_string(),
        )
        .await
        .unwrap();
    let entry4 = OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc(),
        description: desc.clone(),
    }
    .rounded();
    oplog.add(entry4.clone()).await;

    oplog.commit(CommitLevel::Always).await;

    let r_start = oplog.read(last_oplog_idx.next()).await.rounded();
    let r_end = oplog.read(last_oplog_idx.next().next()).await.rounded();
    let r2 = oplog
        .read(last_oplog_idx.next().next().next())
        .await
        .rounded();
    let r3 = oplog
        .read(last_oplog_idx.next().next().next().next())
        .await
        .rounded();
    let r4 = oplog
        .read(last_oplog_idx.next().next().next().next().next())
        .await
        .rounded();

    assert_eq!(r_start, entry_start);
    assert_eq!(r_end, entry_end);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);
    assert_eq!(r4, entry4);

    let entries = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            last_oplog_idx.next(),
            5,
        )
        .await;
    assert_eq!(
        entries
            .into_values()
            .map(|entry| entry.rounded())
            .collect::<Vec<_>>(),
        vec![
            entry_start.clone(),
            entry_end.clone(),
            entry2.clone(),
            entry3.clone(),
            entry4.clone(),
        ]
    );

    let p1 = match entry_end {
        OplogEntry::End {
            response: Some(payload),
            ..
        } => {
            let response = oplog_service
                .download_payload(&owned_agent_id, AgentMode::Durable, payload)
                .await
                .unwrap();
            match response {
                HostResponse::Custom(vnt) => String::from_value(vnt.value()).unwrap(),
                _ => panic!("unexpected response"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p2 = match entry2 {
        OplogEntry::AgentInvocationStarted { payload, .. } => {
            let payload: AgentInvocationPayload = oplog_service
                .download_payload(&owned_agent_id, AgentMode::Durable, payload)
                .await
                .unwrap();
            match payload {
                AgentInvocationPayload::AgentMethod { input, .. } => match input {
                    SchemaValue::Record { fields } => match fields.into_iter().next() {
                        Some(SchemaValue::String(value)) => value,
                        _ => panic!("unexpected element"),
                    },
                    _ => panic!("unexpected data value"),
                },
                _ => panic!("unexpected payload"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p3 = match entry3 {
        OplogEntry::AgentInvocationFinished { result, .. } => {
            let result: AgentInvocationResult = oplog_service
                .download_payload(&owned_agent_id, AgentMode::Durable, result)
                .await
                .unwrap();
            match result {
                AgentInvocationResult::AgentMethod { output } => match output {
                    SchemaValue::Record { fields } => match fields.into_iter().next() {
                        Some(SchemaValue::String(value)) => value,
                        _ => panic!("unexpected element"),
                    },
                    _ => panic!("unexpected data value"),
                },
                _ => panic!("unexpected result"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let (p4, p4_mime) = oplog
        .get_upload_description_payload(desc)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(p1, "response");
    assert_eq!(p2, "request");
    assert_eq!(p3, "response");
    assert_eq!(p4, vec![1, 2, 3]);
    assert_eq!(p4_mime, "application/octet-stream");
}

#[test]
async fn entries_with_large_payload(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let large_payload1 = vec![0u8; 1024 * 1024];
    let large_payload2 = vec![1u8; 1024 * 1024];
    let large_payload3 = vec![2u8; 1024 * 1024];
    let large_payload4 = vec![3u8; 1024 * 1024];

    let last_oplog_idx = oplog.current_oplog_index().await;
    let (start_idx, end_idx) = oplog
        .add_completed_host_call(
            HostFunctionName::Custom("f1".to_string()),
            &HostRequest::Custom("request".to_string().into_typed_schema_value().unwrap()),
            &HostResponse::Custom(large_payload1.clone().into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadRemote,
            None,
        )
        .await
        .unwrap();
    let entry_start = oplog.read(start_idx).await.rounded();
    let entry_end = oplog.read(end_idx).await.rounded();
    let entry2 = oplog
        .add_agent_invocation_started(AgentInvocation::AgentMethod {
            idempotency_key: IdempotencyKey::fresh(),
            method_name: "f2".to_string(),
            input: SchemaValue::Record {
                fields: vec![SchemaValue::Binary(BinaryValuePayload {
                    bytes: large_payload2.clone(),
                    mime_type: None,
                })],
            },
            invocation_context: InvocationContextStack::fresh_rounded(),
            principal: Principal::anonymous(),
        })
        .await
        .unwrap()
        .rounded();
    let entry3 = oplog
        .add_agent_invocation_finished(
            &AgentInvocationResult::AgentMethod {
                output: SchemaValue::Record {
                    fields: vec![SchemaValue::Binary(BinaryValuePayload {
                        bytes: large_payload3.clone(),
                        mime_type: None,
                    })],
                },
            },
            Some("f2".to_string()),
            42,
            ComponentRevision::INITIAL,
        )
        .await
        .unwrap()
        .rounded();

    let desc = oplog
        .create_snapshot_based_update_description(
            ComponentRevision::new(11).unwrap(),
            large_payload4.clone(),
            "application/octet-stream".to_string(),
        )
        .await
        .unwrap();
    let entry4 = OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc(),
        description: desc.clone(),
    }
    .rounded();
    oplog.add(entry4.clone()).await;

    oplog.commit(CommitLevel::Always).await;

    let r_start = oplog.read(last_oplog_idx.next()).await.rounded();
    let r_end = oplog.read(last_oplog_idx.next().next()).await.rounded();
    let r2 = oplog
        .read(last_oplog_idx.next().next().next())
        .await
        .rounded();
    let r3 = oplog
        .read(last_oplog_idx.next().next().next().next())
        .await
        .rounded();
    let r4 = oplog
        .read(last_oplog_idx.next().next().next().next().next())
        .await
        .rounded();

    assert_eq!(r_start, entry_start);
    assert_eq!(r_end, entry_end);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);
    assert_eq!(r4, entry4);

    let entries = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            last_oplog_idx.next(),
            5,
        )
        .await;
    assert_eq!(
        entries
            .into_values()
            .map(|entry| entry.rounded())
            .collect::<Vec<_>>(),
        vec![
            entry_start.clone(),
            entry_end.clone(),
            entry2.clone(),
            entry3.clone(),
            entry4.clone(),
        ]
    );

    let p1 = match entry_end {
        OplogEntry::End {
            response: Some(payload),
            ..
        } => {
            let response = oplog_service
                .download_payload(&owned_agent_id, AgentMode::Durable, payload)
                .await
                .unwrap();
            match response {
                HostResponse::Custom(vnt) => Vec::<u8>::from_value(vnt.value()).unwrap(),
                _ => panic!("unexpected response"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p2 = match entry2 {
        OplogEntry::AgentInvocationStarted { payload, .. } => {
            let payload: AgentInvocationPayload = oplog_service
                .download_payload(&owned_agent_id, AgentMode::Durable, payload)
                .await
                .unwrap();
            match payload {
                AgentInvocationPayload::AgentMethod { input, .. } => match input {
                    SchemaValue::Record { fields } => match fields.into_iter().next() {
                        Some(SchemaValue::Binary(BinaryValuePayload { bytes, .. })) => bytes,
                        _ => panic!("unexpected element"),
                    },
                    _ => panic!("unexpected data value"),
                },
                _ => panic!("unexpected payload"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p3 = match entry3 {
        OplogEntry::AgentInvocationFinished { result, .. } => {
            let result: AgentInvocationResult = oplog_service
                .download_payload(&owned_agent_id, AgentMode::Durable, result)
                .await
                .unwrap();
            match result {
                AgentInvocationResult::AgentMethod { output } => match output {
                    SchemaValue::Record { fields } => match fields.into_iter().next() {
                        Some(SchemaValue::Binary(BinaryValuePayload { bytes, .. })) => bytes,
                        _ => panic!("unexpected element"),
                    },
                    _ => panic!("unexpected data value"),
                },
                _ => panic!("unexpected result"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let (p4, p4_mime) = oplog
        .get_upload_description_payload(desc)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(p1, large_payload1);
    assert_eq!(p2, large_payload2);
    assert_eq!(p3, large_payload3);
    assert_eq!(p4, large_payload4);
    assert_eq!(p4_mime, "application/octet-stream");
}

/// The `PersistNothing` contract for live durable host calls — including the P3 accessor path,
/// which appends its `Start` via `add_start_with_reserved_payload` and its `End` via `add` exactly
/// as simulated here — is enforced at the oplog level, not at the call sites: entries written
/// inside a persist-nothing zone are buffered but never flushed by the call's own
/// `CommitLevel::DurableOnly` commits. They only reach durable storage when a `CommitLevel::Always`
/// commit runs (e.g. the zone-closing `ChangePersistenceLevel`, written via `add_and_commit`),
/// which is intentional: zone contents are observability-only and the replay cursor skips whole
/// persist-nothing zones, never claiming the `Start`/`End` entries inside them.
#[test]
async fn persist_nothing_zone_suppresses_durable_commits_of_live_host_call_entries(
    _tracing: &Tracing,
) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    // Commit thresholds high enough that no threshold-triggered `Always` commit fires during the
    // test (threshold commits flush even inside a persist-nothing zone, to bound memory).
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        100,
        100,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    // Enter a persist-nothing zone the way `set_oplog_persistence_level` does: the zone-begin
    // entry is committed with `CommitLevel::Always` *before* the level switch, so the zone
    // boundary itself is always durable.
    let zone_begin_idx = oplog
        .add_and_commit(OplogEntry::change_persistence_level(
            PersistenceLevel::PersistNothing,
        ))
        .await;
    oplog
        .switch_persistence_level(PersistenceLevel::PersistNothing)
        .await;

    // A live host call inside the zone, using the same primitives as the P3 accessor live path
    // (`execute_access_start` / `CallHandle::complete`): eager `Start` with a reserved request
    // payload, then an `End` referencing it.
    let request = HostRequest::Custom("request".to_string().into_typed_schema_value().unwrap());
    let (start_idx, request_upload) = oplog
        .add_start_with_reserved_payload(request, move |request_payload| OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("f1".to_string()),
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::WriteRemote,
        })
        .await
        .unwrap();
    request_upload.wait().await.unwrap();
    let response = HostResponse::Custom("response".to_string().into_typed_schema_value().unwrap());
    let response_payload = oplog.upload_payload(&response).await.unwrap();
    let end_idx = oplog
        .add(OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: start_idx,
            response: Some(response_payload),
            forced_commit: false,
        })
        .await;

    // The call-site commit (`end_durable_function` commits with `DurableOnly` for remote writes)
    // must not flush anything while the persist-nothing zone is open.
    oplog.commit(CommitLevel::DurableOnly).await;
    let committed = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, start_idx, 10)
        .await;
    check!(
        committed.is_empty(),
        "Start/End written inside a persist-nothing zone must not be durably committed by DurableOnly commits"
    );

    // The buffered entries are still visible through the open oplog (`read_many` merges the
    // uncommitted buffer).
    let buffered = oplog.read_many(start_idx, 2).await;
    check!(matches!(
        buffered.get(&start_idx),
        Some(OplogEntry::Start { .. })
    ));
    check!(matches!(
        buffered.get(&end_idx),
        Some(OplogEntry::End { .. })
    ));

    // Closing the zone (again mirroring `set_oplog_persistence_level`) commits with `Always`,
    // which flushes the zone contents: they become durable as observability-only entries that
    // replay skips wholesale.
    let zone_end_idx = oplog
        .add_and_commit(OplogEntry::change_persistence_level(
            PersistenceLevel::Smart,
        ))
        .await;
    oplog
        .switch_persistence_level(PersistenceLevel::Smart)
        .await;

    let committed = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, zone_begin_idx, 10)
        .await;
    let kinds = committed
        .into_iter()
        .map(|(idx, entry)| (idx, entry.rounded()))
        .collect::<Vec<_>>();
    check!(kinds.len() == 4);
    check!(kinds[0].0 == zone_begin_idx);
    check!(matches!(
        kinds[0].1,
        OplogEntry::ChangePersistenceLevel {
            persistence_level: PersistenceLevel::PersistNothing,
            ..
        }
    ));
    check!(kinds[1].0 == start_idx);
    check!(matches!(kinds[1].1, OplogEntry::Start { .. }));
    check!(kinds[2].0 == end_idx);
    check!(matches!(kinds[2].1, OplogEntry::End { .. }));
    check!(kinds[3].0 == zone_end_idx);
    check!(matches!(
        kinds[3].1,
        OplogEntry::ChangePersistenceLevel {
            persistence_level: PersistenceLevel::Smart,
            ..
        }
    ));
}

#[test]
async fn multilayer_transfers_entries_after_limit_reached_1(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 315, 5, 1, 3, false).await;
}

#[test]
async fn multilayer_transfers_entries_after_limit_reached_2(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 12, 2, 1, 0, false).await;
}

#[test]
async fn multilayer_transfers_entries_after_limit_reached_3(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 10000, 0, 0, 100, false).await;
}

#[test]
async fn blob_multilayer_transfers_entries_after_limit_reached_1(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 315, 5, 1, 3, true).await;
}

#[test]
async fn blob_multilayer_transfers_entries_after_limit_reached_2(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 12, 2, 1, 0, true).await;
}

#[test]
async fn blob_multilayer_transfers_entries_after_limit_reached_3(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 10000, 0, 0, 100, true).await;
}

async fn multilayer_transfers_entries_after_limit_reached(
    use_redis: bool,
    n: u64,
    expected_1: u64,
    expected_2: u64,
    expected_3: u64,
    use_blob: bool,
) {
    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> = if use_redis {
        let pool = RedisPool::configured(&RedisConfig::default())
            .await
            .unwrap();
        Arc::new(RedisIndexedStorage::new(pool))
    } else {
        Arc::new(InMemoryIndexedStorage::new())
    };

    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    let mut entries = Vec::new();

    for i in 0..n {
        // One simple Start entry per iteration; the test only cares about
        // per-entry layer transfer behaviour, not the Start/End pairing.
        let request = oplog
            .upload_payload(&HostRequest::Custom(i.into_typed_schema_value().unwrap()))
            .await
            .unwrap();
        let entry = OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("test-function".to_string()),
            request: Some(request),
            durable_function_type: DurableFunctionType::ReadLocal,
        }
        .rounded();
        oplog.add(entry.clone()).await;
        oplog.commit(CommitLevel::Always).await;
        entries.push(entry);
    }

    let start = Instant::now();
    loop {
        let primary_length = primary_oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
            .length()
            .await;

        let secondary_length = secondary_layer
            .open(&owned_agent_id, AgentMode::Durable)
            .await
            .length()
            .await;
        if primary_length == expected_1 && secondary_length == expected_2 {
            break;
        }
        let elapsed = start.elapsed();
        if elapsed.as_secs() > 120 {
            panic!("Timeout");
        } else {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    debug!("Fetching information to evaluate the test");

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    let all_entries = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::NONE,
            n + 100,
        )
        .await;

    assert_eq!(all_entries.len(), entries.len());
    assert_eq!(primary_length, expected_1);
    assert_eq!(secondary_length, expected_2);
    assert_eq!(tertiary_length, expected_3);
    assert_eq!(
        all_entries.keys().cloned().collect::<Vec<_>>(),
        (1..=n).map(OplogIndex::from_u64).collect::<Vec<_>>()
    );
    check!(all_entries.values().cloned().collect::<Vec<_>>() == entries);
}

#[test]
async fn read_from_archive(_tracing: &Tracing) {
    read_from_archive_impl(false).await;
}

#[test]
async fn blob_read_from_archive(_tracing: &Tracing) {
    read_from_archive_impl(true).await;
}

async fn read_from_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: AgentError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect();

    let initial_oplog_idx = oplog.current_oplog_index().await;

    for entry in &entries {
        oplog.add(entry.clone()).await;
    }
    oplog.commit(CommitLevel::Always).await;
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await;
    oplog.add(uncommitted2.clone()).await;

    entries.push(uncommitted1);
    entries.push(uncommitted2);

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    let first10 = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            initial_oplog_idx.next(),
            10,
        )
        .await;
    let original_first10 = entries.iter().take(10).cloned().collect::<Vec<_>>();

    assert_eq!(first10.into_values().collect::<Vec<_>>(), original_first10);

    let last10 = oplog
        .read_many(oplog.current_oplog_index().await.subtract(10).next(), 10)
        .await
        .into_values()
        .collect::<Vec<_>>();

    let original_last10 = entries.into_iter().rev().take(10).rev().collect::<Vec<_>>();
    assert_eq!(last10, original_last10);
}

#[test]
async fn read_initial_from_archive(_tracing: &Tracing) {
    crate::services::oplog::tests::read_initial_from_archive_impl(false).await;
}

#[test]
async fn blob_read_initial_from_archive(_tracing: &Tracing) {
    crate::services::oplog::tests::read_initial_from_archive_impl(true).await;
}

#[test]
async fn ephemeral_read_initial_from_archive(_tracing: &Tracing) {
    crate::services::oplog::tests::ephemeral_read_initial_from_archive_impl(false).await;
}

#[test]
async fn blob_ephemeral_read_initial_from_archive(_tracing: &Tracing) {
    crate::services::oplog::tests::ephemeral_read_initial_from_archive_impl(true).await;
}

async fn read_initial_from_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let timestamp = Timestamp::now_utc();
    let create_entry = OplogEntry::Create {
        timestamp,
        agent_id: AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "test".to_string(),
        },
        agent_mode: AgentMode::Durable,
        component_revision: ComponentRevision::new(1).unwrap(),
        env: vec![],
        local_agent_config: Vec::new(),
        environment_id,
        created_by: account_id,
        parent: None,
        component_size: 0,
        initial_total_linear_memory_size: 0,
        initial_active_plugins: HashSet::new(),
        original_phantom_id: None,
        instance_id: Uuid::new_v4(),
    }
    .rounded();

    let oplog = oplog_service
        .create(
            &owned_agent_id,
            AgentMode::Durable,
            create_entry.clone(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    // The create entry is in the primary oplog now
    let read1 = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_1 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;

    // Archiving it to the secondary
    let more = MultiLayerOplog::try_archive_blocking(&oplog).await;

    // Reading it again, now it needs to be fetched from the secondary layer
    let read2 = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_2 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;

    // Archiving it to the tertiary
    MultiLayerOplog::try_archive_blocking(&oplog).await;

    // Reading it again, now it needs to be fetched from the tertiary layer
    let read3 = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_3 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;

    assert_eq!(more, Some(true));
    assert_eq!(read1, Some((OplogIndex::INITIAL, create_entry.clone())));
    assert_eq!(read2, Some((OplogIndex::INITIAL, create_entry.clone())));
    assert_eq!(read3, Some((OplogIndex::INITIAL, create_entry)));

    assert_eq!(last_index_1, OplogIndex::INITIAL);
    assert_eq!(last_index_2, OplogIndex::INITIAL);
    assert_eq!(last_index_3, OplogIndex::INITIAL);
}

async fn ephemeral_read_initial_from_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let timestamp = Timestamp::now_utc();
    let create_entry = OplogEntry::Create {
        timestamp,
        agent_id: AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "test".to_string(),
        },
        agent_mode: AgentMode::Ephemeral,
        component_revision: ComponentRevision::new(1).unwrap(),
        env: vec![],
        local_agent_config: Vec::new(),
        environment_id,
        created_by: account_id,
        parent: None,
        component_size: 0,
        initial_total_linear_memory_size: 0,
        initial_active_plugins: HashSet::new(),
        original_phantom_id: None,
        instance_id: Uuid::new_v4(),
    }
    .rounded();

    let oplog = oplog_service
        .create(
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            AgentMetadata {
                agent_mode: AgentMode::Ephemeral,
                ..make_agent_metadata(agent_id.clone(), account_id, environment_id)
            },
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;
    oplog.commit(CommitLevel::Always).await;

    let read_before_archive = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await
        .into_iter()
        .next();
    let more = EphemeralOplog::try_archive_blocking(&oplog).await;
    let read_after_archive = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await
        .into_iter()
        .next();

    assert_eq!(more, Some(false));
    assert_eq!(
        read_before_archive,
        Some((OplogIndex::INITIAL, create_entry.clone()))
    );
    assert_eq!(
        read_after_archive,
        Some((OplogIndex::INITIAL, create_entry))
    );
}

#[test]
async fn write_after_archive(_tracing: &Tracing) {
    write_after_archive_impl(false, Reopen::No).await;
}

#[test]
async fn blob_write_after_archive(_tracing: &Tracing) {
    write_after_archive_impl(true, Reopen::No).await;
}

#[test]
async fn write_after_archive_reopen(_tracing: &Tracing) {
    write_after_archive_impl(false, Reopen::Yes).await;
}

#[test]
async fn blob_write_after_archive_reopen(_tracing: &Tracing) {
    write_after_archive_impl(true, Reopen::Yes).await;
}

#[test]
async fn write_after_archive_reopen_full(_tracing: &Tracing) {
    write_after_archive_impl(false, Reopen::Full).await;
}

#[test]
async fn blob_write_after_archive_reopen_full(_tracing: &Tracing) {
    write_after_archive_impl(true, Reopen::Full).await;
}

#[test]
async fn deleting_worker_fences_in_flight_archive_transfers(_tracing: &Tracing) {
    deleting_worker_fences_in_flight_archive_transfers_impl(AgentMode::Durable).await;
}

#[test]
async fn deleting_ephemeral_worker_fences_in_flight_archive_transfers(_tracing: &Tracing) {
    deleting_worker_fences_in_flight_archive_transfers_impl(AgentMode::Ephemeral).await;
}

async fn deleting_worker_fences_in_flight_archive_transfers_impl(agent_mode: AgentMode) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let (append_started_tx, append_started_rx) = oneshot::channel();
    let release_append = Arc::new(Notify::new());
    let append_finished = Arc::new(Notify::new());
    let (secondary, tertiary, entry_count_limit) = match agent_mode {
        AgentMode::Durable => (
            Arc::new(BlockingArchiveService {
                inner: Arc::new(CompressedOplogArchiveService::new(
                    indexed_storage.clone(),
                    1,
                    RetryConfig::default(),
                )),
                append_started: Arc::new(Mutex::new(Some(append_started_tx))),
                release_append: release_append.clone(),
                append_finished: append_finished.clone(),
            }) as Arc<dyn OplogArchiveService>,
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
                as Arc<dyn OplogArchiveService>,
            1,
        ),
        AgentMode::Ephemeral => (
            Arc::new(CompressedOplogArchiveService::new(
                indexed_storage.clone(),
                1,
                RetryConfig::default(),
            )) as Arc<dyn OplogArchiveService>,
            Arc::new(BlockingArchiveService {
                inner: Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2)),
                append_started: Arc::new(Mutex::new(Some(append_started_tx))),
                release_append: release_append.clone(),
                append_finished: append_finished.clone(),
            }) as Arc<dyn OplogArchiveService>,
            2,
        ),
    };
    let service = Arc::new(MultiLayerOplogService::new(
        primary,
        nev![secondary, tertiary],
        entry_count_limit,
        1,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "delete-during-transfer".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = service
        .open(
            &owned_agent_id,
            agent_mode,
            None,
            AgentMetadata {
                agent_mode,
                ..make_agent_metadata(agent_id, account_id, environment_id)
            },
            default_last_known_status(),
            default_execution_status(agent_mode),
        )
        .await;

    oplog.add(OplogEntry::no_op()).await;
    oplog.commit(CommitLevel::Always).await;
    if agent_mode == AgentMode::Ephemeral {
        EphemeralOplog::try_archive(&oplog)
            .await
            .expect("ephemeral oplog must be archivable");
    }
    tokio::time::timeout(Duration::from_secs(1), append_started_rx)
        .await
        .expect("archive transfer did not start")
        .expect("archive transfer start signal dropped");

    service.delete(&owned_agent_id, agent_mode).await;

    let append_completed = append_finished.notified();
    release_append.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), append_completed)
            .await
            .is_err(),
        "delete did not stop the in-flight archive transfer"
    );

    assert!(
        !service.exists(&owned_agent_id, agent_mode).await,
        "an in-flight archive transfer recreated a deleted oplog"
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Reopen {
    No,
    Yes,
    Full,
}

async fn write_after_archive_impl(use_blob: bool, reopen: Reopen) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let mut primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let mut oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    info!("FIRST OPEN");
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    info!("FIRST OPEN DONE");

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: AgentError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect();

    let initial_oplog_idx = oplog.current_oplog_index().await;

    for entry in &entries {
        oplog.add(entry.clone()).await;
    }
    oplog.commit(CommitLevel::Always).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("initial oplog index: {}", initial_oplog_idx);
    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    let oplog = if reopen == Reopen::Yes {
        drop(oplog);
        oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else if reopen == Reopen::Full {
        drop(oplog);
        primary_oplog_service = Arc::new(
            PrimaryOplogService::new(
                indexed_storage.clone(),
                blob_storage.clone(),
                1,
                1,
                100,
                RetryConfig::default(),
            )
            .await,
        );
        oplog_service = Arc::new(MultiLayerOplogService::new(
            primary_oplog_service.clone(),
            nev![secondary_layer.clone(), tertiary_layer.clone()],
            10,
            10,
        ));
        oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else {
        oplog
    };

    let entries: Vec<OplogEntry> = (100..1000)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: AgentError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect();

    for (n, entry) in entries.iter().enumerate() {
        oplog.add(entry.clone()).await;
        if n % 100 == 0 {
            oplog.commit(CommitLevel::Always).await;
        }
    }
    oplog.commit(CommitLevel::Always).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("initial oplog index: {}", initial_oplog_idx);
    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    let oplog = if reopen == Reopen::Yes {
        drop(oplog);
        oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else if reopen == Reopen::Full {
        drop(oplog);
        primary_oplog_service = Arc::new(
            PrimaryOplogService::new(
                indexed_storage.clone(),
                blob_storage.clone(),
                1,
                1,
                100,
                RetryConfig::default(),
            )
            .await,
        );
        oplog_service = Arc::new(MultiLayerOplogService::new(
            primary_oplog_service.clone(),
            nev![secondary_layer.clone(), tertiary_layer.clone()],
            10,
            10,
        ));
        oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else {
        oplog
    };

    oplog
        .add(
            OplogEntry::Error {
                timestamp,
                error: AgentError::Unknown("last".to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded(),
        )
        .await;
    oplog.commit(CommitLevel::Always).await;
    drop(oplog);

    let entry1 = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await;
    let entry2 = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::from_u64(100),
            1,
        )
        .await;
    let entry3 = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::from_u64(1000),
            1,
        )
        .await;
    let entry4 = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::from_u64(1001),
            1,
        )
        .await;

    assert_eq!(entry1.len(), 1);
    assert_eq!(entry2.len(), 1);
    assert_eq!(entry3.len(), 1);
    assert_eq!(entry4.len(), 1);

    assert_eq!(
        entry1.get(&OplogIndex::INITIAL).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: AgentError::Unknown("0".to_string()),
            retry_from: OplogIndex::NONE,
            inside_atomic_region: false,
            retry_policy_state: None,
        }
        .rounded()
    );
    assert_eq!(
        entry2.get(&OplogIndex::from_u64(100)).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: AgentError::Unknown("99".to_string()),
            retry_from: OplogIndex::NONE,
            inside_atomic_region: false,
            retry_policy_state: None,
        }
        .rounded()
    );
    assert_eq!(
        entry3.get(&OplogIndex::from_u64(1000)).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: AgentError::Unknown("999".to_string()),
            retry_from: OplogIndex::NONE,
            inside_atomic_region: false,
            retry_policy_state: None,
        }
        .rounded()
    );
    assert_eq!(
        entry4.get(&OplogIndex::from_u64(1001)).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: AgentError::Unknown("last".to_string()),
            retry_from: OplogIndex::NONE,
            inside_atomic_region: false,
            retry_policy_state: None,
        }
        .rounded()
    );
}

#[test]
async fn empty_layer_gets_deleted(_tracing: &Tracing) {
    empty_layer_gets_deleted_impl(false).await;
}

#[test]
async fn blob_empty_layer_gets_deleted(_tracing: &Tracing) {
    empty_layer_gets_deleted_impl(true).await;
}

async fn empty_layer_gets_deleted_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    // As we add 100 entries at once, and that exceeds the limit, we expect that all entries have
    // been moved to the secondary layer. By doing this 10 more times, we end up having all entries
    // in the tertiary layer.

    for _ in 0..10 {
        let timestamp = Timestamp::now_utc();
        let entries: Vec<OplogEntry> = (0..100)
            .map(|i| {
                OplogEntry::Error {
                    timestamp,
                    error: AgentError::Unknown(i.to_string()),
                    retry_from: OplogIndex::NONE,
                    inside_atomic_region: false,
                    retry_policy_state: None,
                }
                .rounded()
            })
            .collect();

        for entry in &entries {
            oplog.add(entry.clone()).await;
        }
        oplog.commit(CommitLevel::Always).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    let primary_exists = primary_oplog_service
        .exists(&owned_agent_id, AgentMode::Durable)
        .await;
    let secondary_exists = secondary_layer
        .exists(&owned_agent_id, AgentMode::Durable)
        .await;
    let tertiary_exists = tertiary_layer
        .exists(&owned_agent_id, AgentMode::Durable)
        .await;

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    assert_eq!(primary_length, 0);
    assert_eq!(secondary_length, 0);
    assert_eq!(tertiary_length, 1);

    assert!(!primary_exists);
    assert!(!secondary_exists);
    assert!(tertiary_exists);
}

#[test]
async fn scheduled_archive(_tracing: &Tracing) {
    scheduled_archive_impl(false).await;
}

#[test]
async fn blob_scheduled_archive(_tracing: &Tracing) {
    scheduled_archive_impl(true).await;
}

async fn scheduled_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        1000, // no transfer will occur by reaching limit in this test
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: AgentError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect();

    // Adding 100 entries to the primary oplog, schedule archive and immediately drop the oplog
    let archive_result = {
        let oplog = oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await;
        for entry in &entries {
            oplog.add(entry.clone()).await;
        }
        oplog.commit(CommitLevel::Always).await;

        let result = MultiLayerOplog::try_archive(&oplog).await;
        drop(oplog);
        result
    };

    let last_oplog_index_1 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    assert_eq!(primary_length, 0);
    assert_eq!(secondary_length, 1);
    assert_eq!(tertiary_length, 0);
    assert_eq!(archive_result, Some(true));

    let last_oplog_index_2 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;

    assert_eq!(last_oplog_index_1, last_oplog_index_2);

    // Calling archive again
    let archive_result2 = {
        let oplog = oplog_service
            .open(
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await;
        let result = MultiLayerOplog::try_archive(&oplog).await;
        drop(oplog);
        result
    };

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;
    let tertiary_length = tertiary_layer
        .open(&owned_agent_id, AgentMode::Durable)
        .await
        .length()
        .await;

    info!("primary_length 2: {}", primary_length);
    info!("secondary_length 2: {}", secondary_length);
    info!("tertiary_length 2: {}", tertiary_length);

    assert_eq!(primary_length, 0);
    assert_eq!(secondary_length, 0);
    assert_eq!(tertiary_length, 1);
    assert_eq!(archive_result2, Some(false));

    let last_oplog_index_3 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;

    assert_eq!(last_oplog_index_2, last_oplog_index_3);
}

#[test]
async fn multilayer_scan_for_component(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));

    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        1000, // no transfer will occur by reaching limit in this test
        10,
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    // Adding some workers
    let mut primary_workers = Vec::new();
    let mut secondary_workers = Vec::new();
    let mut tertiary_workers = Vec::new();
    for i in 0..100 {
        let agent_id = AgentId {
            component_id,
            agent_id: format!("worker-{i}"),
        };
        let create_entry = OplogEntry::create(
            agent_id.clone(),
            AgentMode::Durable,
            ComponentRevision::new(1).unwrap(),
            Vec::new(),
            environment_id,
            account_id,
            None,
            100,
            100,
            HashSet::new(),
            Vec::new(),
            None,
            Uuid::new_v4(),
        );

        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let oplog = oplog_service
            .create(
                &owned_agent_id,
                AgentMode::Durable,
                create_entry,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await;

        debug!("Created {agent_id}");
        match i % 3 {
            0 => primary_workers.push(agent_id),
            1 => {
                secondary_workers.push(agent_id.clone());
                debug!("Archiving {agent_id} to secondary layer");
                MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!("Adding more oplog entries to primary");
                    oplog
                        .add_and_commit(OplogEntry::log(
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await;
                }
            }
            2 => {
                tertiary_workers.push(agent_id.clone());
                debug!("Archiving {agent_id} to secondary layer");
                let r = MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!(
                        "Adding more oplog entries to primary going to be moved to the secondary layer"
                    );
                    oplog
                        .add_and_commit(OplogEntry::log(
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await;
                }

                debug!("[{r:?}] => archiving {agent_id} to tertiary layer");
                MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!("Adding more oplog entries to primary");
                    oplog
                        .add_and_commit(OplogEntry::log(
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await;
                }
            }
            _ => unreachable!(),
        }
    }

    debug!(
        "Created {}/{}/{} workers, waiting for background processes",
        primary_workers.len(),
        secondary_workers.len(),
        tertiary_workers.len()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut cursor = ScanCursor::default();
    let mut result = Vec::new();
    let page_size = 10;
    loop {
        let (new_cursor, ids) = oplog_service
            .scan_for_component(&environment_id, &component_id, None, cursor, page_size)
            .await
            .unwrap();
        debug!("Got {} elements, new cursor is {}", ids.len(), new_cursor);
        result.extend(ids);
        if new_cursor.is_finished() {
            break;
        } else {
            cursor = new_cursor;
        }
    }

    assert_eq!(result.len(), 100);
}

/// Ephemeral workers in a multi-layer oplog service live only in the lower
/// (archive) layers - the multi-layer service writes their create entry straight
/// to the first lower layer rather than to the primary. This test verifies that
/// `scan_for_component` discovers such ephemeral workers through the archive-only
/// lower-layer scan path and that mode filtering is honored across layers.
#[test]
async fn multilayer_scan_for_component_ephemeral(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));

    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        // High entry-count limit so no background transfer between lower layers
        // happens during the test - the ephemeral workers stay in the first
        // lower (archive) layer.
        1000,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    let create_worker = async |mode: AgentMode, name: String| -> OwnedAgentId {
        let agent_id = AgentId {
            component_id,
            agent_id: name,
        };
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let create_entry = OplogEntry::create(
            agent_id.clone(),
            mode,
            ComponentRevision::new(1).unwrap(),
            Vec::new(),
            environment_id,
            account_id,
            None,
            100,
            100,
            HashSet::new(),
            Vec::new(),
            None,
            Uuid::now_v7(),
        );
        let oplog = oplog_service
            .create(
                &owned_agent_id,
                mode,
                create_entry,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(mode),
            )
            .await;
        oplog.commit(CommitLevel::Always).await;
        owned_agent_id
    };

    let mut durable = Vec::new();
    for i in 0..20 {
        durable.push(create_worker(AgentMode::Durable, format!("dur-{i}")).await);
    }
    let mut ephemeral = Vec::new();
    for i in 0..20 {
        ephemeral.push(create_worker(AgentMode::Ephemeral, format!("eph-{i}")).await);
    }

    // Give any background processes a chance to settle.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let drain = async |modes: Option<AgentMode>| {
        let mut cursor = ScanCursor::default();
        let mut acc: Vec<OwnedAgentId> = Vec::new();
        // Use a small page size so pagination crosses layers and mode boundaries.
        let page_size = 7;
        loop {
            let (next_cursor, ids) = oplog_service
                .scan_for_component(&environment_id, &component_id, modes, cursor, page_size)
                .await
                .unwrap();
            acc.extend(ids);
            if next_cursor.is_finished() {
                break;
            }
            cursor = next_cursor;
        }
        acc.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
        acc
    };

    let mut expected_durable = durable.clone();
    expected_durable.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut expected_ephemeral = ephemeral.clone();
    expected_ephemeral.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut expected_both: Vec<OwnedAgentId> = durable.into_iter().chain(ephemeral).collect();
    expected_both.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));

    assert_eq!(drain(Some(AgentMode::Ephemeral)).await, expected_ephemeral);
    assert_eq!(drain(Some(AgentMode::Durable)).await, expected_durable);
    assert_eq!(drain(None).await, expected_both);
}

/// Reproducer for the oplog unique key violation panic during recovery.
///
/// The race is in `OpenOplogs::get_or_open`: when two tasks concurrently call
/// it for the same worker_id, both can observe `entry.initial == true` and both
/// execute `decrement_strong_count`. This can over-decrement the Arc refcount,
/// causing premature drop, the Weak becoming un-upgradeable, cache eviction,
/// and creation of a **second** oplog instance for the same worker. Two instances
/// means two independent `last_committed_idx` counters, leading to duplicate
/// INSERT attempts and a unique key violation in SQLite.
#[test]
async fn concurrent_get_or_open_does_not_cause_unique_key_violation(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().expect("Cannot create temp dir");
    let database = tempdir
        .path()
        .join("indexed.db")
        .to_string_lossy()
        .into_owned();
    let config = golem_common::config::DbSqliteConfig {
        database,
        max_connections: 10,
        foreign_keys: false,
    };
    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> =
        Arc::new(SqliteIndexedStorage::configured(&config).await.unwrap());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            100,
            100,
            100,
            RetryConfig::default(),
        )
        .await,
    );

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "concurrent-test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &worker_id);

    // First, create the oplog with an initial entry so it exists in SQLite
    let initial_oplog = oplog_service
        .create(
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::jump(OplogRegion {
                start: OplogIndex::from_u64(0),
                end: OplogIndex::from_u64(0),
            }),
            make_agent_metadata(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    initial_oplog.commit(CommitLevel::Always).await;
    drop(initial_oplog);

    // Wait for the weak reference to become invalid so the cache entry is evicted
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Now simulate the race: many concurrent tasks open the same oplog and write to it.
    // This exercises the `initial` flag race in `get_or_open`.
    // If two tasks get different oplog instances due to the race, they'll have
    // independent `last_committed_idx` counters and produce duplicate ids on INSERT,
    // triggering SQLite's UNIQUE constraint violation.
    let num_tasks = 20;
    let num_iterations = 50;
    let barrier = Arc::new(tokio::sync::Barrier::new(num_tasks));
    let failure_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let mut handles = Vec::new();
    for _task_id in 0..num_tasks {
        let oplog_service = oplog_service.clone();
        let owned_agent_id = owned_agent_id.clone();
        let worker_id = worker_id.clone();
        let barrier = barrier.clone();
        let _failure_count = failure_count.clone();

        handles.push(tokio::spawn(async move {
            for _iteration in 0..num_iterations {
                // Synchronize all tasks to maximize contention on get_or_open
                barrier.wait().await;

                let oplog = oplog_service
                    .open(
                        &owned_agent_id,
                        AgentMode::Durable,
                        None,
                        make_agent_metadata(worker_id.clone(), account_id, environment_id),
                        default_last_known_status(),
                        default_execution_status(AgentMode::Durable),
                    )
                    .await;

                // Each task adds an entry and commits. If two tasks ended up with
                // different oplog instances (due to the get_or_open race), they'll
                // have independent last_committed_idx and produce duplicate ids,
                // causing a unique key violation on commit.
                oplog.add(OplogEntry::suspend()).await;
                // Use fallible_add pattern: commit can panic on unique key violation;
                // we use the Oplog trait method directly and let it propagate.
                oplog.commit(CommitLevel::Always).await;

                tokio::task::yield_now().await;
            }
        }));
    }

    for handle in handles {
        match handle.await {
            Ok(()) => {}
            Err(e) => {
                if e.is_panic() {
                    let panic_msg = if let Some(s) = e.into_panic().downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    if panic_msg.contains("unique key violation")
                        || panic_msg.contains("Key already exists")
                        || panic_msg.contains("UNIQUE constraint failed")
                    {
                        failure_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        panic!("Unexpected panic: {panic_msg}");
                    }
                }
            }
        }
    }

    let failures = failure_count.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        failures, 0,
        "Got {failures} unique key violations from concurrent oplog access — \
         the get_or_open initial flag race caused duplicate oplog instances"
    );
}

// ---------------------------------------------------------------------------
// Step 8: Scan-cursor mode-bit encoding helpers
// ---------------------------------------------------------------------------

#[test]
fn scan_cursor_helpers_initial_cursor_starts_in_durable_phase() {
    // A freshly-constructed ScanCursor has cursor == 0 and the active
    // phase must be `Durable` with `Ephemeral` queued as next.
    let (active, next) = scan_modes(None, 0);
    assert_eq!(active, AgentMode::Durable);
    assert_eq!(next, Some(AgentMode::Ephemeral));
    assert_eq!(cursor_value(0), 0);
}

#[test]
fn scan_cursor_helpers_high_bit_marks_ephemeral_phase() {
    let (active, next) = scan_modes(None, SCAN_CURSOR_EPHEMERAL_BIT);
    assert_eq!(active, AgentMode::Ephemeral);
    assert_eq!(next, None);
    // The high bit must not leak into the storage cursor value.
    assert_eq!(cursor_value(SCAN_CURSOR_EPHEMERAL_BIT), 0);
}

#[test]
fn scan_cursor_helpers_value_mask_strips_only_high_bit() {
    let raw = SCAN_CURSOR_EPHEMERAL_BIT | 0x42;
    let (active, next) = scan_modes(None, raw);
    assert_eq!(active, AgentMode::Ephemeral);
    assert_eq!(next, None);
    assert_eq!(cursor_value(raw), 0x42);
}

#[test]
fn scan_cursor_helpers_explicit_single_mode_does_not_phase_transition() {
    let (active, next) = scan_modes(Some(AgentMode::Durable), 0);
    assert_eq!(active, AgentMode::Durable);
    assert_eq!(next, None);

    let (active, next) = scan_modes(Some(AgentMode::Ephemeral), 0);
    assert_eq!(active, AgentMode::Ephemeral);
    assert_eq!(next, None);

    // The high bit is only meaningful for `modes == None`; with an explicit
    // mode the helper must ignore it.
    let (active, next) = scan_modes(Some(AgentMode::Durable), SCAN_CURSOR_EPHEMERAL_BIT);
    assert_eq!(active, AgentMode::Durable);
    assert_eq!(next, None);
}

#[test]
fn scan_cursor_helpers_durable_phase_in_progress_is_round_trip_stable() {
    // While the durable phase is still in progress (cursor_val != 0) the
    // returned cursor must keep the high bit clear and round-trip back to
    // the same active mode.
    let cur = next_scan_cursor(123, AgentMode::Durable, Some(AgentMode::Ephemeral), 2);
    assert_eq!(cur.layer, 2);
    assert_eq!(cur.cursor & SCAN_CURSOR_EPHEMERAL_BIT, 0);
    assert_eq!(cursor_value(cur.cursor), 123);
    let (active, next) = scan_modes(None, cur.cursor);
    assert_eq!(active, AgentMode::Durable);
    assert_eq!(next, Some(AgentMode::Ephemeral));
}

#[test]
fn scan_cursor_helpers_durable_phase_finished_advances_to_ephemeral() {
    // When the durable phase finishes (cursor_val == 0) and there is a next
    // phase, the helper must hand control over to that phase by setting
    // the high bit. The resulting cursor must NOT be `is_finished`.
    let cur = next_scan_cursor(0, AgentMode::Durable, Some(AgentMode::Ephemeral), 0);
    assert_eq!(cur.cursor, SCAN_CURSOR_EPHEMERAL_BIT);
    assert_eq!(cur.layer, 0);
    assert!(!cur.is_finished());
    let (active, next) = scan_modes(None, cur.cursor);
    assert_eq!(active, AgentMode::Ephemeral);
    assert_eq!(next, None);
}

#[test]
fn scan_cursor_helpers_ephemeral_phase_in_progress_keeps_high_bit_set() {
    let cur = next_scan_cursor(7, AgentMode::Ephemeral, None, 0);
    assert_eq!(
        cur.cursor & SCAN_CURSOR_EPHEMERAL_BIT,
        SCAN_CURSOR_EPHEMERAL_BIT
    );
    assert_eq!(cursor_value(cur.cursor), 7);
    assert!(!cur.is_finished());
    let (active, next) = scan_modes(None, cur.cursor);
    assert_eq!(active, AgentMode::Ephemeral);
    assert_eq!(next, None);
}

#[test]
fn scan_cursor_helpers_both_phases_finished_yields_terminal_cursor() {
    // After the ephemeral phase (the last one) finishes, the returned
    // cursor must compare equal to the default and be `is_finished`.
    let cur = next_scan_cursor(0, AgentMode::Ephemeral, None, 0);
    assert_eq!(cur, ScanCursor::default());
    assert!(cur.is_finished());
}

// ---------------------------------------------------------------------------
// Step 8: Durable / ephemeral oplog isolation
// ---------------------------------------------------------------------------

#[test]
async fn durable_and_ephemeral_oplogs_are_isolated_for_same_agent_id(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "isolation-test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let durable_create = OplogEntry::create(
        agent_id.clone(),
        AgentMode::Durable,
        ComponentRevision::new(1).unwrap(),
        Vec::new(),
        environment_id,
        account_id,
        None,
        100,
        100,
        HashSet::new(),
        Vec::new(),
        None,
        Uuid::now_v7(),
    )
    .rounded();
    let ephemeral_create = OplogEntry::create(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(2).unwrap(),
        Vec::new(),
        environment_id,
        account_id,
        None,
        100,
        100,
        HashSet::new(),
        Vec::new(),
        None,
        Uuid::now_v7(),
    )
    .rounded();

    let durable_oplog = oplog_service
        .create(
            &owned_agent_id,
            AgentMode::Durable,
            durable_create.clone(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    let ephemeral_oplog = oplog_service
        .create(
            &owned_agent_id,
            AgentMode::Ephemeral,
            ephemeral_create.clone(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;
    durable_oplog.commit(CommitLevel::Always).await;
    ephemeral_oplog.commit(CommitLevel::Always).await;

    // Both namespaces report the oplog exists, independently.
    assert!(
        oplog_service
            .exists(&owned_agent_id, AgentMode::Durable)
            .await
    );
    assert!(
        oplog_service
            .exists(&owned_agent_id, AgentMode::Ephemeral)
            .await
    );

    // Each namespace returns its own initial entry, not the other's.
    let durable_first = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_values()
        .next()
        .expect("expected one durable entry");
    let ephemeral_first = oplog_service
        .read(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await
        .into_values()
        .next()
        .expect("expected one ephemeral entry");
    assert_eq!(durable_first, durable_create);
    assert_eq!(ephemeral_first, ephemeral_create);
    assert_ne!(durable_first, ephemeral_first);

    // Deleting one namespace must not affect the other.
    oplog_service
        .delete(&owned_agent_id, AgentMode::Durable)
        .await;
    assert!(
        !oplog_service
            .exists(&owned_agent_id, AgentMode::Durable)
            .await
    );
    assert!(
        oplog_service
            .exists(&owned_agent_id, AgentMode::Ephemeral)
            .await
    );
}

// ---------------------------------------------------------------------------
// Step 8: Multi-mode scan_for_component pagination
// ---------------------------------------------------------------------------

async fn make_workers(
    oplog_service: &PrimaryOplogService,
    environment_id: EnvironmentId,
    component_id: ComponentId,
    account_id: AccountId,
    mode: AgentMode,
    n: usize,
    name_prefix: &str,
) -> Vec<OwnedAgentId> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let agent_id = AgentId {
            component_id,
            agent_id: format!("{name_prefix}-{i}"),
        };
        let create_entry = OplogEntry::create(
            agent_id.clone(),
            mode,
            ComponentRevision::new(1).unwrap(),
            Vec::new(),
            environment_id,
            account_id,
            None,
            100,
            100,
            HashSet::new(),
            Vec::new(),
            None,
            Uuid::now_v7(),
        );
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let oplog = oplog_service
            .create(
                &owned_agent_id,
                mode,
                create_entry,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(mode),
            )
            .await;
        oplog.commit(CommitLevel::Always).await;
        out.push(owned_agent_id);
    }
    out
}

#[test]
async fn scan_for_component_only_returns_matching_mode_when_specified(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    let durable = make_workers(
        &oplog_service,
        environment_id,
        component_id,
        account_id,
        AgentMode::Durable,
        3,
        "dur",
    )
    .await;
    let ephemeral = make_workers(
        &oplog_service,
        environment_id,
        component_id,
        account_id,
        AgentMode::Ephemeral,
        4,
        "eph",
    )
    .await;

    let drain = async |modes: Option<AgentMode>| {
        let mut cursor = ScanCursor::default();
        let mut acc: Vec<OwnedAgentId> = Vec::new();
        loop {
            let (next_cursor, ids) = oplog_service
                .scan_for_component(&environment_id, &component_id, modes, cursor, 100)
                .await
                .unwrap();
            acc.extend(ids);
            if next_cursor.is_finished() {
                break;
            }
            cursor = next_cursor;
        }
        acc.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
        acc
    };

    let mut expected_durable = durable.clone();
    expected_durable.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut expected_ephemeral = ephemeral.clone();
    expected_ephemeral.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut expected_both: Vec<OwnedAgentId> = durable.into_iter().chain(ephemeral).collect();
    expected_both.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));

    assert_eq!(drain(Some(AgentMode::Durable)).await, expected_durable);
    assert_eq!(drain(Some(AgentMode::Ephemeral)).await, expected_ephemeral);
    assert_eq!(drain(None).await, expected_both);
}

#[test]
async fn scan_for_component_paginates_across_mode_boundary(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    let durable = make_workers(
        &oplog_service,
        environment_id,
        component_id,
        account_id,
        AgentMode::Durable,
        7,
        "dur",
    )
    .await;
    let ephemeral = make_workers(
        &oplog_service,
        environment_id,
        component_id,
        account_id,
        AgentMode::Ephemeral,
        5,
        "eph",
    )
    .await;
    let total = durable.len() + ephemeral.len();

    // Use a very small page size so pagination must cross the durable→ephemeral boundary.
    let page_size = 3u64;
    let mut cursor = ScanCursor::default();
    let mut all_ids: Vec<OwnedAgentId> = Vec::new();
    let mut iterations = 0;
    let mut saw_ephemeral_phase = false;
    let mut saw_durable_phase = false;
    loop {
        iterations += 1;
        // The cursor passed in must encode the active mode for the next page.
        let (active_in, _) = scan_modes(None, cursor.cursor);
        match active_in {
            AgentMode::Durable => saw_durable_phase = true,
            AgentMode::Ephemeral => saw_ephemeral_phase = true,
        }

        let (next_cursor, ids) = oplog_service
            .scan_for_component(&environment_id, &component_id, None, cursor, page_size)
            .await
            .unwrap();

        // Each page is bounded by the storage backend's contract; we never get
        // more than `page_size` items per page from PrimaryOplogService.
        assert!(
            ids.len() as u64 <= page_size,
            "page returned {} > {page_size} items",
            ids.len()
        );

        all_ids.extend(ids);

        if next_cursor.is_finished() {
            break;
        }
        cursor = next_cursor;

        // Defensive: prevent runaway loops if pagination is broken.
        assert!(
            iterations < (total as u64) + 4,
            "pagination did not terminate after {iterations} iterations"
        );
    }

    // Both phases must have been visited at least once during the scan.
    assert!(saw_durable_phase, "durable scanning phase never observed");
    assert!(
        saw_ephemeral_phase,
        "ephemeral scanning phase never observed"
    );

    // No duplicates across pages and no losses.
    let mut sorted = all_ids.clone();
    sorted.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    sorted.dedup();
    assert_eq!(sorted.len(), all_ids.len(), "scan produced duplicate ids");
    assert_eq!(all_ids.len(), total);

    let mut expected: Vec<OwnedAgentId> = durable.into_iter().chain(ephemeral).collect();
    expected.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut got = all_ids;
    got.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    assert_eq!(got, expected);
}

#[test]
async fn scan_for_component_with_no_workers_terminates_immediately(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;

    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    let mut cursor = ScanCursor::default();
    let mut iterations = 0;
    loop {
        iterations += 1;
        let (next_cursor, ids) = oplog_service
            .scan_for_component(&environment_id, &component_id, None, cursor, 10)
            .await
            .unwrap();
        assert!(ids.is_empty());
        if next_cursor.is_finished() {
            break;
        }
        cursor = next_cursor;
        // Even with both-modes scanning the empty case must finish quickly:
        // it should take at most one extra iteration to advance past the
        // empty durable phase into the ephemeral phase, and one more to
        // observe ephemeral is also empty.
        assert!(
            iterations < 4,
            "empty scan did not terminate within 3 iterations"
        );
    }
}

/// A large request reserved with [`OplogOps::add_start_with_reserved_payload`] is stored externally,
/// and its deferred blob upload is made durable by the leaf oplog's commit barrier even when the
/// caller never awaits the returned [`PendingUpload`].
#[test]
async fn reserved_large_request_is_durable_via_commit_barrier(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let large_payload = vec![7u8; 1024 * 1024];
    let request = HostRequest::Custom(large_payload.clone().into_typed_schema_value().unwrap());

    let last_oplog_idx = oplog.current_oplog_index().await;
    // Deliberately drop the returned `PendingUpload` without awaiting it: the commit barrier in
    // `append` must still make the deferred external blob durable before the referencing `Start` is
    // committed.
    let (start_idx, _pending) = oplog
        .add_start_with_reserved_payload(request, |request_payload| OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("f".to_string()),
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::ReadRemote,
        })
        .await
        .unwrap();
    assert_eq!(start_idx, last_oplog_idx.next());

    oplog.commit(CommitLevel::Always).await;

    // Read back from the service (storage), so the payload reference carries no in-memory cache and
    // the download must hit blob storage.
    let entries = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, start_idx, 1)
        .await;
    let entry = entries.into_values().next().expect("Start entry present");
    let payload = match entry {
        OplogEntry::Start {
            request: Some(payload),
            ..
        } => {
            assert!(
                matches!(payload, OplogPayload::External { .. }),
                "a large reserved request must be stored externally"
            );
            payload
        }
        other => panic!("unexpected entry: {other:?}"),
    };

    let downloaded: HostRequest = oplog_service
        .download_payload(&owned_agent_id, AgentMode::Durable, payload)
        .await
        .unwrap();
    match downloaded {
        HostRequest::Custom(vnt) => {
            assert_eq!(Vec::<u8>::from_value(vnt.value()).unwrap(), large_payload);
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

/// A small request reserved with [`OplogOps::add_start_with_reserved_payload`] is stored inline, and
/// its [`PendingUpload`] is a no-op (nothing to upload).
#[test]
async fn reserved_small_request_stays_inline(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let request = HostRequest::Custom("request".into_typed_schema_value().unwrap());

    let last_oplog_idx = oplog.current_oplog_index().await;
    let (start_idx, pending) = oplog
        .add_start_with_reserved_payload(request, |request_payload| OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("f".to_string()),
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::ReadRemote,
        })
        .await
        .unwrap();
    assert_eq!(start_idx, last_oplog_idx.next());
    // Inline payloads are already durable: waiting is a no-op.
    pending.wait().await.unwrap();

    oplog.commit(CommitLevel::Always).await;

    let entries = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, start_idx, 1)
        .await;
    let entry = entries.into_values().next().expect("Start entry present");
    let payload = match entry {
        OplogEntry::Start {
            request: Some(payload),
            ..
        } => {
            assert!(
                matches!(payload, OplogPayload::SerializedInline { .. }),
                "a small reserved request must be stored inline"
            );
            payload
        }
        other => panic!("unexpected entry: {other:?}"),
    };

    let downloaded: HostRequest = oplog_service
        .download_payload(&owned_agent_id, AgentMode::Durable, payload)
        .await
        .unwrap();
    match downloaded {
        HostRequest::Custom(vnt) => {
            assert_eq!(String::from_value(vnt.value()).unwrap(), "request");
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

fn reserved_start_entry_builder(
    function_name: &str,
) -> impl FnOnce(OplogPayload<HostRequest>) -> OplogEntry + Send + 'static {
    let function_name = HostFunctionName::Custom(function_name.to_string());
    move |request_payload| OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name,
        request: Some(request_payload),
        durable_function_type: DurableFunctionType::ReadRemote,
    }
}

fn reserved_start_function_name(entry: &OplogEntry) -> String {
    match entry {
        OplogEntry::Start {
            function_name: HostFunctionName::Custom(name),
            ..
        } => name.clone(),
        other => panic!("unexpected entry: {other:?}"),
    }
}

/// Reserved-start on a multi-layer oplog delegates to the primary leaf (which owns the
/// `Start`-ordering critical section) and keeps the multi-layer's exposed last oplog index in
/// lockstep with the indices the primary assigned.
#[test]
async fn multilayer_reserved_start_delegates_to_primary_and_tracks_last_index(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer, tertiary_layer],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let base = oplog.current_oplog_index().await;

    let (first_idx, first_pending) = oplog
        .add_start_with_reserved_payload(
            HostRequest::Custom("first".into_typed_schema_value().unwrap()),
            reserved_start_entry_builder("first"),
        )
        .await
        .unwrap();
    let (second_idx, second_pending) = oplog
        .add_start_with_reserved_payload(
            HostRequest::Custom("second".into_typed_schema_value().unwrap()),
            reserved_start_entry_builder("second"),
        )
        .await
        .unwrap();

    // Delegated to the primary in initiation order...
    assert_eq!(first_idx, base.next());
    assert_eq!(second_idx, first_idx.next());
    // ...and the multi-layer's exposed last index followed the primary's assignments.
    assert_eq!(oplog.current_oplog_index().await, second_idx);

    first_pending.wait().await.unwrap();
    second_pending.wait().await.unwrap();
    oplog.commit(CommitLevel::Always).await;

    let entries = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, first_idx, 2)
        .await;
    assert_eq!(
        entries
            .values()
            .map(reserved_start_function_name)
            .collect::<Vec<_>>(),
        vec!["first".to_string(), "second".to_string()]
    );
}

/// Reserved-start on an ephemeral oplog uploads the request payload eagerly: the payload blob is
/// already durable in storage when the call returns, before any commit. Ephemeral oplogs are never
/// replayed, so — unlike replayable oplogs — reserved-start makes no cross-call initiation-order
/// promise here, and this test intentionally does not assert one.
#[test]
async fn ephemeral_reserved_start_uploads_payload_eagerly(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer],
        10,
        10,
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "ephemeral-reserved".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = OplogEntry::create(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        Vec::new(),
        environment_id,
        account_id,
        None,
        100,
        100,
        HashSet::new(),
        Vec::new(),
        None,
        Uuid::new_v4(),
    )
    .rounded();
    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = oplog_service
        .create(
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry,
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    // The primary's max_payload_size is 100 bytes, so this request is stored externally.
    let large_payload = vec![7u8; 64 * 1024];
    let request = HostRequest::Custom(large_payload.into_typed_schema_value().unwrap());
    let serialized_request = golem_common::serialization::serialize(&request).unwrap();

    let (start_idx, pending) = oplog
        .add_start_with_reserved_payload(request, reserved_start_entry_builder("large"))
        .await
        .unwrap();
    // The upload already happened eagerly; waiting is a no-op.
    pending.wait().await.unwrap();

    // Without any commit, the entry is visible through the uncommitted buffer and the payload
    // blob is already durable: download the raw bytes directly from storage (bypassing the
    // in-memory cache embedded in the returned payload reference).
    let entry = oplog.read(start_idx).await;
    let (payload_id, md5_hash) = match &entry {
        OplogEntry::Start {
            request:
                Some(OplogPayload::External {
                    payload_id,
                    md5_hash,
                    ..
                }),
            ..
        } => (payload_id.clone(), md5_hash.clone()),
        other => panic!("expected an externally stored request, got: {other:?}"),
    };
    let downloaded = oplog
        .download_raw_payload(payload_id, md5_hash)
        .await
        .unwrap();
    assert_eq!(downloaded, serialized_request);
}

/// Smoke test for reserved-start through the actual production oplog stack, in the exact wrapper
/// order composed in `lib.rs`: `RateLimited(Forwarding(MultiLayer(Primary)))`. Verifies initiation
/// ordering, large-payload external storage with durable download, and inline small payloads all
/// survive the full stack.
#[test]
async fn reserved_start_through_production_stack_smoke(_tracing: &Tracing) {
    use crate::services::component::ComponentService;
    use crate::services::oplog::plugin::{ForwardingOplogService, OplogProcessorPlugin};
    use crate::services::oplog::rate_limited::RateLimitedOplogService;
    use crate::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
    use golem_common::model::InvocationStatus;
    use golem_common::model::application::ApplicationId;
    use golem_common::model::component::InstalledPlugin;
    use golem_service_base::model::component::Component;

    /// The worker has no active oplog processor plugins, so the forwarding layer never consults
    /// the plugin service; every method is unreachable.
    #[derive(Debug)]
    struct NoPluginsOplogProcessorPlugin;

    #[async_trait::async_trait]
    impl OplogProcessorPlugin for NoPluginsOplogProcessorPlugin {
        async fn resolve_target(
            &self,
            _environment_id: EnvironmentId,
            _plugin: &InstalledPlugin,
        ) -> Result<AgentId, WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }

        async fn send(
            &self,
            _worker_metadata: AgentMetadata,
            _plugin: &InstalledPlugin,
            _target_agent_id: &AgentId,
            _initial_oplog_index: OplogIndex,
            _entries: Vec<OplogEntry>,
        ) -> Result<(), WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }

        async fn invalidate_target(
            &self,
            _environment_id: EnvironmentId,
            _plugin: &InstalledPlugin,
        ) {
            unreachable!("no active plugins in this test")
        }

        async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError> {
            Ok(())
        }

        async fn is_local(&self, _agent_id: &AgentId) -> Result<bool, WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }

        async fn lookup_invocation_status(
            &self,
            _environment_id: EnvironmentId,
            _target_agent_id: &AgentId,
            _caller_account_id: AccountId,
            _idempotency_key: &IdempotencyKey,
        ) -> Result<InvocationStatus, WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }
    }

    /// Component metadata is only fetched when a plugin flush happens; with no active plugins
    /// every method is unreachable.
    struct NoComponentsComponentService;

    #[async_trait::async_trait]
    impl ComponentService for NoComponentsComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }

        async fn get_metadata(
            &self,
            _component_id: ComponentId,
            _forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            unreachable!("no active plugins in this test")
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            Vec::new()
        }

        async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
    }

    /// Unlimited limits ([`AtomicResourceEntry::new`] defaults to the unlimited oplog write
    /// rate), so the rate-limited wrapper admits every write immediately.
    struct UnlimitedResourceLimits;

    #[async_trait::async_trait]
    impl ResourceLimits for UnlimitedResourceLimits {
        async fn initialize_account(
            &self,
            _account_id: AccountId,
        ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
            Ok(Arc::new(AtomicResourceEntry::new(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                u64::MAX,
                AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            )))
        }
    }

    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default()),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let multilayer_oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service,
        nev![secondary_layer, tertiary_layer],
        10,
        10,
    ));
    let forwarding_oplog_service = Arc::new(ForwardingOplogService::new(
        multilayer_oplog_service,
        Arc::new(NoPluginsOplogProcessorPlugin),
        Arc::new(NoComponentsComponentService),
        100,
        Duration::from_secs(3600),
    ));
    let oplog_service: Arc<dyn OplogService> = Arc::new(RateLimitedOplogService::new(
        forwarding_oplog_service,
        Arc::new(UnlimitedResourceLimits),
    ));

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "production-stack".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let base = oplog.current_oplog_index().await;

    // The primary's max_payload_size is 100 bytes: the first request goes external (deferred
    // upload behind the commit barrier), the second stays inline. Deliberately drop the large
    // call's `PendingUpload` without awaiting it: the leaf commit barrier must still make the
    // external blob durable.
    let large_payload = vec![7u8; 64 * 1024];
    let (large_idx, _pending) = oplog
        .add_start_with_reserved_payload(
            HostRequest::Custom(large_payload.clone().into_typed_schema_value().unwrap()),
            reserved_start_entry_builder("large"),
        )
        .await
        .unwrap();
    let (small_idx, small_pending) = oplog
        .add_start_with_reserved_payload(
            HostRequest::Custom("small".into_typed_schema_value().unwrap()),
            reserved_start_entry_builder("small"),
        )
        .await
        .unwrap();

    // Initiation order is preserved through the full production wrapper stack.
    assert_eq!(large_idx, base.next());
    assert_eq!(small_idx, large_idx.next());
    small_pending.wait().await.unwrap();

    oplog.commit(CommitLevel::Always).await;

    // Read back through the service stack (no in-memory cache).
    let entries = oplog_service
        .read(&owned_agent_id, AgentMode::Durable, large_idx, 2)
        .await;
    assert_eq!(
        entries
            .values()
            .map(reserved_start_function_name)
            .collect::<Vec<_>>(),
        vec!["large".to_string(), "small".to_string()]
    );

    let large_stored = match entries.get(&large_idx).unwrap() {
        OplogEntry::Start {
            request: Some(payload),
            ..
        } => {
            assert!(
                matches!(payload, OplogPayload::External { .. }),
                "a large reserved request must be stored externally"
            );
            payload.clone()
        }
        other => panic!("unexpected entry: {other:?}"),
    };
    let downloaded: HostRequest = oplog_service
        .download_payload(&owned_agent_id, AgentMode::Durable, large_stored)
        .await
        .unwrap();
    match downloaded {
        HostRequest::Custom(vnt) => {
            assert_eq!(Vec::<u8>::from_value(vnt.value()).unwrap(), large_payload);
        }
        other => panic!("unexpected request: {other:?}"),
    }

    let small_stored = match entries.get(&small_idx).unwrap() {
        OplogEntry::Start {
            request: Some(payload),
            ..
        } => {
            assert!(
                matches!(payload, OplogPayload::SerializedInline { .. }),
                "a small reserved request must be stored inline"
            );
            payload.clone()
        }
        other => panic!("unexpected entry: {other:?}"),
    };
    let downloaded: HostRequest = oplog_service
        .download_payload(&owned_agent_id, AgentMode::Durable, small_stored)
        .await
        .unwrap();
    match downloaded {
        HostRequest::Custom(vnt) => {
            assert_eq!(String::from_value(vnt.value()).unwrap(), "small");
        }
        other => panic!("unexpected request: {other:?}"),
    }
}
