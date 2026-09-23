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
use crate::services::oplog::multilayer::{
    OplogArchive, OplogArchiveService, transfer_between_lower_layers,
};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
    WriterId,
};
use assert2::check;
use bytes::Bytes;
use futures::FutureExt;
use futures::stream::BoxStream;
use golem_common::config::RedisConfig;
use golem_common::model::ShardEpoch;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, OwnerKind, Principal};
use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
use golem_common::model::component::ComponentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{AgentError, LogLevel, OplogErrorKind};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AgentFingerprint, AgentMetadata, AgentStatusRecord, IdempotencyKey, OwnedAgentId,
};
use golem_common::model::{AgentInvocationPayload, RetryConfig};
use golem_common::redis::RedisPool;
use golem_common::schema::{BinaryValuePayload, FromSchema, IntoTypedSchemaValue, SchemaValue};
use golem_common::tracing::{TracingConfig, init_tracing};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult,
};
use nonempty_collections::nev;
use std::collections::{HashSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};
use test_r::{test, test_dep};
use tokio::sync::{Mutex, Notify, oneshot};
use tracing::{debug, info};
use uuid::Uuid;

macro_rules! create_oplog_entry {
    ($agent_id:expr, $agent_mode:expr, $component_revision:expr, $env:expr,
     $environment_id:expr, $created_by:expr, $parent:expr, $component_size:expr,
     $memory_size:expr, $plugins:expr, $config:expr, $phantom_id:expr, $instance_id:expr $(,)?) => {
        OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: $agent_id,
            owner_kind: OwnerKind::ComponentAgent,
            agent_mode: $agent_mode,
            component_revision: $component_revision,
            env: $env,
            environment_id: $environment_id,
            created_by: $created_by,
            parent: $parent,
            component_size: $component_size,
            initial_total_linear_memory_size: $memory_size,
            initial_active_plugins: $plugins,
            local_agent_config: $config,
            original_phantom_id: $phantom_id,
            instance_id: $instance_id,
        }))
    };
}

fn create_test_entry(
    agent_id: AgentId,
    agent_mode: AgentMode,
    component_revision: ComponentRevision,
    environment_id: EnvironmentId,
    created_by: AccountId,
    instance_id: Uuid,
) -> OplogEntry {
    create_oplog_entry!(
        agent_id,
        agent_mode,
        component_revision,
        Vec::new(),
        environment_id,
        created_by,
        None,
        100,
        100,
        HashSet::new(),
        Vec::new(),
        None,
        instance_id,
    )
}

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

async fn assert_panics<T>(future: impl Future<Output = T>) {
    assert!(AssertUnwindSafe(future).catch_unwind().await.is_err());
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
    async fn read_source(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        self.calls.read.fetch_add(1, Ordering::Relaxed);
        self.inner.read_source(id, mode, idx, n).await
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
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        self.calls.archive_read.fetch_add(1, Ordering::Relaxed);
        self.inner.read_source(idx, n).await
    }
    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        self.calls.append.fetch_add(1, Ordering::Relaxed);
        self.inner.append(chunk).await
    }
    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
        self.inner.verify_persisted(entries).await
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
        owner_kind: OwnerKind::ComponentAgent,
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

fn invocation_wallet_pin() -> InvocationWalletPin {
    InvocationWalletPin {
        wallet_token: WalletVersionToken {
            wallet_id_hash: [0x42; 32],
            generation: 7,
        },
        pinned_card_ids: Vec::new(),
        scope_card_id: None,
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

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner
            .read_source(owned_agent_id, agent_mode, idx, n)
            .await
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
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read_source(idx, n).await
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        if let Some(sender) = self.append_started.lock().await.take() {
            let _ = sender.send(());
        }
        self.release_append.notified().await;
        let result = self.inner.append(chunk).await;
        self.append_finished.notify_one();
        result
    }

    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
        self.inner.verify_persisted(entries).await
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

#[derive(Debug)]
struct TransferTestArchive {
    role: &'static str,
    entries: std::sync::Mutex<BTreeMap<OplogIndex, OplogEntry>>,
    events: Arc<std::sync::Mutex<Vec<String>>>,
    fail_append: bool,
    fail_verification: bool,
}

impl TransferTestArchive {
    fn new(
        role: &'static str,
        entries: BTreeMap<OplogIndex, OplogEntry>,
        events: Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            role,
            entries: std::sync::Mutex::new(entries),
            events,
            fail_append: false,
            fail_verification: false,
        }
    }

    fn record(&self, operation: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("{}.{}", self.role, operation));
    }
}

#[async_trait]
impl OplogArchive for TransferTestArchive {
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.record("read");
        if n == 0 {
            return BTreeMap::new();
        }
        self.entries
            .lock()
            .unwrap()
            .range(idx..=idx.range_end(n))
            .map(|(index, entry)| (*index, entry.clone()))
            .collect()
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        self.record("append");
        assert!(!self.fail_append, "injected archive append failure");
        self.entries.lock().unwrap().extend(chunk.iter().cloned());
        chunk.len() as u64
    }

    async fn verify_persisted(&self, expected: &[(OplogIndex, OplogEntry)]) {
        self.record("verify");
        assert!(
            !self.fail_verification,
            "injected persisted verification failure"
        );
        let entries = self.entries.lock().unwrap();
        assert!(
            expected
                .iter()
                .all(|(index, entry)| entries.get(index) == Some(entry)),
            "persisted entries differ"
        );
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.entries
            .lock()
            .unwrap()
            .last_key_value()
            .map(|(index, _)| *index)
            .unwrap_or(OplogIndex::NONE)
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.record("drop");
        let mut entries = self.entries.lock().unwrap();
        let retained = entries.split_off(&last_dropped_id.next());
        let dropped = entries.len() as u64;
        *entries = retained;
        dropped
    }

    async fn length(&self) -> u64 {
        self.entries.lock().unwrap().len() as u64
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.current_oplog_index().await
    }
}

#[derive(Debug, Clone, Copy)]
enum InjectedAppendFailure {
    None,
    IndeterminateBeforeWrite,
    TransientBeforeWrite,
    PermanentBeforeWrite,
    CommitThenIndeterminate,
    CommitDifferentThenIndeterminate,
    CommitPrefixThenIndeterminate,
    /// Refuses the write as a stale epoch would, naming the epoch one above whatever was
    /// asserted. Simulates the storage fencing the reconciliation probe `retry_oplog_append`
    /// repeats after a genuine indeterminate-write mismatch.
    Fenced,
}

impl InjectedAppendFailure {
    fn before_write_error(
        self,
        key: &str,
        shard_epoch: Option<ShardEpoch>,
    ) -> Option<IndexedStorageError> {
        match self {
            Self::IndeterminateBeforeWrite => Some(IndexedStorageError::Indeterminate(
                "injected connection loss".to_string(),
            )),
            Self::TransientBeforeWrite => Some(IndexedStorageError::Transient(
                "injected pool timeout".to_string(),
            )),
            Self::PermanentBeforeWrite => Some(IndexedStorageError::Other(
                "injected permanent failure".to_string(),
            )),
            Self::Fenced => Some(IndexedStorageError::Fenced {
                key: key.to_string(),
                expected: shard_epoch.unwrap_or_default(),
                actual: shard_epoch.map(|epoch| ShardEpoch(epoch.0 + 1)),
                writer_conflict: false,
            }),
            _ => None,
        }
    }

    fn after_write_result(self) -> Result<(), IndexedStorageError> {
        match self {
            Self::None => Ok(()),
            Self::CommitThenIndeterminate
            | Self::CommitDifferentThenIndeterminate
            | Self::CommitPrefixThenIndeterminate => Err(IndexedStorageError::Indeterminate(
                "injected connection loss".to_string(),
            )),
            Self::IndeterminateBeforeWrite
            | Self::TransientBeforeWrite
            | Self::PermanentBeforeWrite
            | Self::Fenced => unreachable!(),
        }
    }
}

/// `IndexedStorage` decorator counting read-type operations, used to prove at
/// the storage level that fresh oplog construction performs no reads before
/// its first append.
#[derive(Debug, Default)]
pub(crate) struct ReadCountingIndexedStorage {
    inner: InMemoryIndexedStorage,
    reads: AtomicUsize,
    discard_compressed_appends: bool,
    read_error: Option<IndexedStorageError>,
    read_failures: StdMutex<VecDeque<IndexedStorageError>>,
    hidden_reads: AtomicUsize,
    append_failures: StdMutex<VecDeque<InjectedAppendFailure>>,
    append_many_failures: StdMutex<VecDeque<InjectedAppendFailure>>,
    append_attempts: AtomicUsize,
    append_many_attempts: AtomicUsize,
    append_many_batch_ptr: AtomicUsize,
    append_many_batch_changed: AtomicBool,
    drop_prefix_started: StdMutex<Option<oneshot::Sender<()>>>,
    release_drop_prefix: Option<Arc<Notify>>,
}

impl ReadCountingIndexedStorage {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Every `drop_prefix` waits for `release`; the first one signals `started` when it arrives.
    fn blocking_drop_prefix(started: oneshot::Sender<()>, release: Arc<Notify>) -> Self {
        Self {
            drop_prefix_started: StdMutex::new(Some(started)),
            release_drop_prefix: Some(release),
            ..Self::default()
        }
    }

    fn discarding_compressed_appends() -> Self {
        Self {
            discard_compressed_appends: true,
            ..Self::default()
        }
    }

    fn failing_reads(error: IndexedStorageError) -> Self {
        Self {
            read_error: Some(error),
            ..Self::default()
        }
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    pub(crate) fn reset(&self) {
        self.reads.store(0, Ordering::Relaxed)
    }

    fn reset_append_observations(&self) {
        self.append_attempts.store(0, Ordering::Relaxed);
        self.append_many_attempts.store(0, Ordering::Relaxed);
        self.append_many_batch_ptr.store(0, Ordering::Relaxed);
        self.append_many_batch_changed
            .store(false, Ordering::Relaxed);
    }

    fn count_read(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
    }

    fn inject_append_failure(&self, failure: InjectedAppendFailure) {
        self.append_failures.lock().unwrap().push_back(failure);
    }

    fn inject_append_many_failure(&self, failure: InjectedAppendFailure) {
        self.inject_append_many_failures([failure]);
    }

    fn inject_append_many_failures(
        &self,
        failures: impl IntoIterator<Item = InjectedAppendFailure>,
    ) {
        self.append_many_failures.lock().unwrap().extend(failures);
    }

    fn append_attempts(&self) -> usize {
        self.append_attempts.load(Ordering::Relaxed)
    }

    fn append_many_attempts(&self) -> usize {
        self.append_many_attempts.load(Ordering::Relaxed)
    }

    fn append_many_reused_batch(&self) -> bool {
        !self.append_many_batch_changed.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl IndexedStorage for ReadCountingIndexedStorage {
    async fn set_key_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        shard_epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        self.inner
            .set_key_epoch(svc_name, api_name, namespace, key, shard_epoch)
            .await
    }

    async fn delete_with_epoch(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.inner
            .delete_with_epoch(svc_name, api_name, namespace, key, expected_epoch)
            .await
    }

    async fn number_of_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> Result<u8, IndexedStorageError> {
        self.inner.number_of_replicas(svc_name, api_name).await
    }

    async fn wait_for_replicas(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        replicas: u8,
        timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        self.inner
            .wait_for_replicas(svc_name, api_name, replicas, timeout)
            .await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        self.count_read();
        self.inner.exists(svc_name, api_name, namespace, key).await
    }

    async fn scan_stable(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<crate::storage::indexed::ScanResume>,
        count: u64,
    ) -> Result<(Option<crate::storage::indexed::ScanResume>, Vec<String>), IndexedStorageError>
    {
        self.count_read();
        self.inner
            .scan_stable(svc_name, api_name, namespace, prefix, resume, count)
            .await
    }

    async fn append(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        mut value: Vec<u8>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.append_attempts.fetch_add(1, Ordering::Relaxed);
        if self.discard_compressed_appends
            && matches!(&namespace, IndexedStorageNamespace::CompressedOpLog { .. })
        {
            return Ok(());
        }
        let failure = self
            .append_failures
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(InjectedAppendFailure::None);
        if let Some(error) = failure.before_write_error(key, shard_epoch) {
            return Err(error);
        }
        if matches!(
            failure,
            InjectedAppendFailure::CommitDifferentThenIndeterminate
        ) {
            value.push(0);
        }
        self.inner
            .append(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                id,
                value,
                shard_epoch,
            )
            .await?;
        failure.after_write_result()
    }

    async fn append_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, Bytes)]>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        self.append_many_attempts.fetch_add(1, Ordering::Relaxed);
        if self.discard_compressed_appends
            && matches!(namespace, IndexedStorageNamespace::CompressedOpLog { .. })
        {
            return Ok(());
        }
        let ptr = pairs.as_ptr() as usize;
        let previous_ptr = self
            .append_many_batch_ptr
            .compare_exchange(0, ptr, Ordering::Relaxed, Ordering::Relaxed)
            .unwrap_or_else(|previous| previous);
        if previous_ptr != 0 && previous_ptr != ptr {
            self.append_many_batch_changed
                .store(true, Ordering::Relaxed);
        }

        let failure = self
            .append_many_failures
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(InjectedAppendFailure::None);
        if let Some(error) = failure.before_write_error(key, shard_epoch) {
            return Err(error);
        }
        let pairs = if matches!(
            failure,
            InjectedAppendFailure::CommitDifferentThenIndeterminate
        ) {
            let mut different = pairs.to_vec();
            let first = different.first_mut().expect("non-empty injected batch");
            let mut value = first.1.to_vec();
            value.push(0);
            first.1 = Bytes::from(value);
            different.into()
        } else if matches!(
            failure,
            InjectedAppendFailure::CommitPrefixThenIndeterminate
        ) {
            vec![pairs.first().expect("non-empty injected batch").clone()].into()
        } else {
            pairs
        };
        self.inner
            .append_many(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                pairs,
                shard_epoch,
            )
            .await?;
        failure.after_write_result()
    }

    async fn length(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        self.count_read();
        self.inner.length(svc_name, api_name, namespace, key).await
    }

    async fn delete(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        self.inner.delete(svc_name, api_name, namespace, key).await
    }

    async fn read(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        self.count_read();
        if let Some(error) = &self.read_error {
            return Err(error.clone());
        }
        if let Some(error) = self.read_failures.lock().unwrap().pop_front() {
            return Err(error);
        }
        if self
            .hidden_reads
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
        {
            return Ok(Vec::new());
        }
        self.inner
            .read(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                start_id,
                end_id,
            )
            .await
    }

    async fn first(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        self.count_read();
        self.inner
            .first(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn last(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        self.count_read();
        self.inner
            .last(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn last_id(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        self.count_read();
        self.inner
            .last_id(svc_name, api_name, entity_name, namespace, key)
            .await
    }

    async fn closest(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        self.count_read();
        self.inner
            .closest(svc_name, api_name, entity_name, namespace, key, id)
            .await
    }

    async fn drop_prefix(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), IndexedStorageError> {
        if let Some(release) = &self.release_drop_prefix {
            let started = self.drop_prefix_started.lock().unwrap().take();
            if let Some(started) = started {
                let _ = started.send(());
            }
            release.notified().await;
        }
        self.inner
            .drop_prefix(svc_name, api_name, namespace, key, last_dropped_id)
            .await
    }
}

/// `BlobStorage` decorator counting read-type operations and optionally failing a raw write.
#[derive(Debug)]
pub(crate) struct ReadCountingBlobStorage {
    inner: InMemoryBlobStorage,
    reads: AtomicUsize,
    puts: AtomicUsize,
    fail_put: Option<usize>,
    pause_put: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    pause_read: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl ReadCountingBlobStorage {
    pub(crate) fn new() -> Self {
        Self {
            inner: InMemoryBlobStorage::new(),
            reads: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
            fail_put: None,
            pause_put: std::sync::Mutex::new(None),
            pause_read: std::sync::Mutex::new(None),
        }
    }

    fn failing_on_put(fail_put: usize) -> Self {
        Self {
            inner: InMemoryBlobStorage::new(),
            reads: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
            fail_put: Some(fail_put),
            pause_put: std::sync::Mutex::new(None),
            pause_read: std::sync::Mutex::new(None),
        }
    }

    fn pause_next_put(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self.pause_put.lock().unwrap() = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) fn pause_next_read(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self.pause_read.lock().unwrap() = Some((started_tx, release_rx));
        (started_rx, release_tx)
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    pub(crate) fn reset(&self) {
        self.reads.store(0, Ordering::Relaxed)
    }

    fn count_read(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
    }
}

#[async_trait]
impl BlobStorage for ReadCountingBlobStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, anyhow::Error> {
        self.count_read();
        let pause = self.pause_read.lock().unwrap().take();
        if let Some((started, release)) = pause {
            let _ = started.send(());
            let _ = release.await;
        }
        self.inner
            .get_raw(target_label, op_label, namespace, path)
            .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, anyhow::Error>>>, anyhow::Error> {
        self.count_read();
        self.inner
            .get_stream(target_label, op_label, namespace, path)
            .await
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, anyhow::Error> {
        self.count_read();
        self.inner
            .get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), anyhow::Error> {
        let put = self.puts.fetch_add(1, Ordering::Relaxed) + 1;
        let pause = self.pause_put.lock().unwrap().take();
        if let Some((started, release)) = pause {
            let _ = started.send(());
            let _ = release.await;
        }
        if self.fail_put == Some(put) {
            return Err(anyhow::anyhow!("injected blob write failure {put}"));
        }
        self.inner
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, anyhow::Error>, Error = anyhow::Error>,
    ) -> Result<(), anyhow::Error> {
        self.inner
            .put_stream(target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), anyhow::Error> {
        self.inner
            .delete(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), anyhow::Error> {
        self.inner
            .create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, anyhow::Error> {
        self.count_read();
        self.inner
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, anyhow::Error> {
        self.inner
            .delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, anyhow::Error> {
        self.count_read();
        self.inner
            .exists(target_label, op_label, namespace, path)
            .await
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
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();

    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = service
        .create(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
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
        .read_exact(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));
    assert_eq!(calls.read.load(Ordering::Relaxed), 1);

    let archive_reads = calls.archive_read.load(Ordering::Relaxed);
    assert_panics(oplog.read_exact(OplogIndex::INITIAL, 2)).await;
    assert_eq!(calls.archive_read.load(Ordering::Relaxed), archive_reads);

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
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();

    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
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
        .read_exact(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));
    assert_eq!(calls.read.load(Ordering::Relaxed), 1);

    for _ in 0..2 {
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(3),
                EphemeralOplog::try_archive_blocking(&oplog)
            )
            .await
            .expect("blocking archival hung after the last movable layer was empty"),
            Some(false)
        );
    }
    assert_eq!(
        service
            .read_exact(
                &owned_agent_id,
                AgentMode::Ephemeral,
                OplogIndex::INITIAL,
                1
            )
            .await
            .get(&OplogIndex::INITIAL),
        Some(&create_entry)
    );
    drop(oplog);
}

/// Storage-level regression guard for `CompressedOplogArchive::open_fresh`:
/// even if the archive-service wrapper reports no reads, a future change that
/// adds an eager read to the fresh constructor must fail this test.
#[test]
async fn fresh_ephemeral_create_with_compressed_layers_does_not_read_storage(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
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
    let lower1: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage.clone(),
        1,
        RetryConfig::default(),
    ));
    let lower2: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage.clone(),
        2,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary.clone(), nev![lower1, lower2], 10, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fresh-ephemeral-compressed-storage".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();

    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    indexed_storage.reset();
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    assert_eq!(indexed_storage.reads(), 0);

    let entries = service
        .read_exact(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));
    assert!(indexed_storage.reads() > 0);

    drop(oplog);
}

#[test]
async fn primary_fresh_ephemeral_create_does_not_read_storage(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = PrimaryOplogService::new(
        indexed_storage.clone(),
        Arc::new(InMemoryBlobStorage::new()),
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fresh-ephemeral-primary-storage".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();
    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;

    indexed_storage.reset();
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    assert_eq!(indexed_storage.reads(), 0);

    let entries = service
        .read_exact(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));
    assert!(indexed_storage.reads() > 0);

    drop(oplog);
}

#[test]
async fn staged_oplog_is_hidden_through_flush_and_published_without_cache_or_blob_aliases(
    _tracing: &Tracing,
) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            16,
            RetryConfig::default(),
        )
        .await,
    );
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage,
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary.clone(), nev![archive], 1, 1);
    let agent = AgentId {
        component_id: ComponentId::new(),
        agent_id: "staged".into(),
    };
    let owned = OwnedAgentId::new(EnvironmentId::new(), &agent);
    let metadata = make_agent_metadata(agent.clone(), AccountId::new(), owned.environment_id);
    let create = create_test_entry(
        agent.clone(),
        AgentMode::Durable,
        ComponentRevision::INITIAL,
        owned.environment_id,
        metadata.created_by,
        metadata.fingerprint.0,
    )
    .rounded();
    let stage_id = Uuid::new_v4();
    assert!(
        service
            .create_staged(&owned, AgentMode::Ephemeral, stage_id, metadata.clone())
            .await
            .is_err()
    );
    assert!(
        service
            .publish_staged(&owned, AgentMode::Ephemeral, stage_id, OplogIndex::INITIAL)
            .await
            .is_err()
    );
    let stage = service
        .create_staged(&owned, AgentMode::Durable, stage_id, metadata.clone())
        .await
        .unwrap();
    assert!(
        !service
            .staged_exists(&owned, AgentMode::Durable, stage_id)
            .await
            .unwrap()
    );
    // A crashed attempt leaves its committed stage behind. A fresh attempt must neither
    // enumerate it as an agent nor reuse its contents when publishing the same target.
    let orphan_id = Uuid::new_v4();
    let orphan = service
        .create_staged(&owned, AgentMode::Durable, orphan_id, metadata.clone())
        .await
        .unwrap();
    orphan.add(create.clone()).await.unwrap();
    orphan.add(OplogEntry::no_op(None).rounded()).await.unwrap();
    orphan.commit(CommitLevel::Always).await.unwrap();
    drop(orphan);
    let entries = [
        create.clone(),
        OplogEntry::suspend().rounded(),
        OplogEntry::no_op(None).rounded(),
    ];
    for entry in &entries {
        stage.add(entry.clone()).await.unwrap();
        stage.commit(CommitLevel::Always).await.unwrap();
        assert!(
            service
                .staged_exists(&owned, AgentMode::Durable, stage_id)
                .await
                .unwrap()
        );
        assert!(!service.exists(&owned, AgentMode::Durable).await);
        assert_eq!(
            service.get_last_index(&owned, AgentMode::Durable).await,
            OplogIndex::NONE
        );
        assert!(
            service
                .scan_for_component(
                    &owned.environment_id,
                    &agent.component_id,
                    Some(AgentMode::Durable),
                    ScanCursor::default(),
                    100,
                )
                .await
                .unwrap()
                .1
                .is_empty()
        );
    }
    assert_eq!(
        stage
            .read_exact(OplogIndex::INITIAL, 3)
            .await
            .into_values()
            .collect::<Vec<_>>(),
        entries
    );
    let payload = stage.upload_raw_payload(vec![7; 4096]).await.unwrap();
    let RawOplogPayload::External {
        payload_id,
        md5_hash,
    } = payload
    else {
        panic!("expected external payload")
    };
    drop(stage);
    assert!(
        service
            .publish_staged(
                &owned,
                AgentMode::Durable,
                stage_id,
                OplogIndex::from_u64(3)
            )
            .await
            .unwrap()
    );
    assert!(
        !service
            .staged_exists(&owned, AgentMode::Durable, stage_id)
            .await
            .unwrap()
    );
    assert_eq!(
        service
            .read_exact(&owned, AgentMode::Durable, OplogIndex::INITIAL, 3)
            .await
            .into_values()
            .collect::<Vec<_>>(),
        entries
    );
    service
        .discard_staged(&owned, AgentMode::Durable, orphan_id)
        .await
        .unwrap();
    let reopened = service
        .open(
            &mut service.lock_lifecycle(&owned.agent_id).await,
            &owned,
            AgentMode::Durable,
            None,
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert_eq!(
        reopened.current_oplog_index().await,
        OplogIndex::from_u64(3)
    );
    assert_eq!(
        reopened
            .download_raw_payload(payload_id.clone(), md5_hash.clone())
            .await
            .unwrap(),
        vec![7; 4096]
    );

    let losing_id = Uuid::new_v4();
    let losing = service
        .create_staged(&owned, AgentMode::Durable, losing_id, metadata)
        .await
        .unwrap();
    losing.add(create).await.unwrap();
    losing.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(losing.current_oplog_index().await, OplogIndex::INITIAL);
    assert_eq!(
        reopened.current_oplog_index().await,
        OplogIndex::from_u64(3)
    );
    drop(losing);
    assert!(
        !service
            .publish_staged(&owned, AgentMode::Durable, losing_id, OplogIndex::INITIAL)
            .await
            .unwrap()
    );
    MultiLayerOplog::try_archive_blocking(&reopened).await;
    assert_eq!(
        primary.get_last_index(&owned, AgentMode::Durable).await,
        OplogIndex::NONE
    );
    assert!(
        !service
            .publish_staged(&owned, AgentMode::Durable, losing_id, OplogIndex::INITIAL)
            .await
            .unwrap()
    );
    service
        .discard_staged(&owned, AgentMode::Durable, losing_id)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .download_raw_payload(payload_id, md5_hash)
            .await
            .unwrap(),
        vec![7; 4096]
    );
    assert_eq!(
        service
            .read_exact(&owned, AgentMode::Durable, OplogIndex::INITIAL, 3)
            .await
            .into_values()
            .collect::<Vec<_>>(),
        entries
    );
}

#[test]
async fn primary_uses_agent_mode_commit_threshold(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let service = PrimaryOplogService::new(
        indexed_storage,
        Arc::new(InMemoryBlobStorage::new()),
        100,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let service = &service;

    let open = |agent_mode, agent_name: &str| {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: agent_name.into(),
        };
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
        metadata.agent_mode = agent_mode;
        async move {
            service
                .open(
                    &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
                    &owned_agent_id,
                    agent_mode,
                    None,
                    metadata,
                    default_last_known_status(),
                    default_execution_status(agent_mode),
                    None,
                )
                .await
        }
    };

    let durable = open(AgentMode::Durable, "durable-threshold").await;
    let ephemeral = open(AgentMode::Ephemeral, "ephemeral-threshold").await;
    for oplog in [&durable, &ephemeral] {
        oplog.add(OplogEntry::suspend().rounded()).await.unwrap();
        oplog.add(OplogEntry::exited().rounded()).await.unwrap();
    }

    assert_eq!(durable.length().await, 0);
    assert_eq!(ephemeral.length().await, 2);
}

/// Storage-level zero-read contract for the blob archive backend, whose fresh
/// construction diverges most from the checked path (a real `exists`/`list_dir`
/// call is bypassed).
#[test]
async fn fresh_ephemeral_create_with_blob_layers_does_not_read_storage(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let blob_storage = Arc::new(ReadCountingBlobStorage::new());
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
    let lower1: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1));
    let lower2: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let service = MultiLayerOplogService::new(primary.clone(), nev![lower1, lower2], 10, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fresh-ephemeral-blob-storage".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();

    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    indexed_storage.reset();
    blob_storage.reset();
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    assert_eq!(indexed_storage.reads(), 0);
    assert_eq!(blob_storage.reads(), 0);

    let entries = service
        .read_exact(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            1,
        )
        .await;
    assert_eq!(entries.get(&OplogIndex::INITIAL), Some(&create_entry));
    assert!(blob_storage.reads() > 0);

    drop(oplog);
}

fn append_reconciliation_retry_config() -> RetryConfig {
    RetryConfig {
        max_attempts: 3,
        min_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        multiplier: 1.0,
        max_jitter_factor: None,
    }
}

async fn append_reconciliation_service(
    indexed_storage: Arc<ReadCountingIndexedStorage>,
) -> Arc<PrimaryOplogService> {
    Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            Arc::new(InMemoryBlobStorage::new()),
            100,
            100,
            100,
            append_reconciliation_retry_config(),
        )
        .await,
    )
}

async fn create_append_reconciliation_oplog(
    service: &PrimaryOplogService,
    name: &str,
) -> Arc<dyn Oplog> {
    create_append_reconciliation_oplog_with_epoch(service, name, None).await
}

async fn create_append_reconciliation_oplog_with_epoch(
    service: &PrimaryOplogService,
    name: &str,
    shard_epoch: Option<ShardEpoch>,
) -> Arc<dyn Oplog> {
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: name.to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::NONE,
                    end: OplogIndex::NONE,
                },
            ),
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            shard_epoch,
        )
        .await
}

#[test]
async fn lifecycle_reader_blocks_delete_and_late_drop_cannot_remove_replacement(
    _tracing: &Tracing,
) {
    let service = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            100,
            100,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "lifecycle".into(),
    };
    let id = OwnedAgentId::new(environment_id, &agent_id);
    let metadata = make_agent_metadata(agent_id.clone(), account_id, environment_id);
    let original = OplogEntry::no_op(None).rounded();
    let mut read_guard = service.lock_lifecycle(&agent_id).await;
    let old = service
        .create_fresh(
            &mut read_guard,
            &id,
            AgentMode::Durable,
            original.clone(),
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let stale = old.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let delete = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move {
            let lock = service.lock_lifecycle(&id.agent_id);
            tokio::pin!(lock);
            assert!(futures::poll!(&mut lock).is_pending());
            entered_tx.send(()).unwrap();
            let mut guard = lock.await;
            old.stop_and_wait().await.unwrap();
            service
                .delete(&mut guard, &id, AgentMode::Durable, None)
                .await
                .unwrap();
            let next_lifecycle = service.lock_lifecycle(&id.agent_id);
            tokio::pin!(next_lifecycle);
            assert!(futures::poll!(&mut next_lifecycle).is_pending());
        }
    });
    entered_rx.await.unwrap();
    assert_eq!(stale.read(OplogIndex::INITIAL).await, original);
    drop(read_guard);
    delete.await.unwrap();

    let mut guard = service.lock_lifecycle(&agent_id).await;
    assert!(!service.exists(&id, AgentMode::Durable).await);
    let replacement_entry = OplogEntry::jump(
        None,
        OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(7),
        },
    )
    .rounded();
    let replacement = service
        .create_fresh(
            &mut guard,
            &id,
            AgentMode::Durable,
            replacement_entry.clone(),
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    drop(stale);
    let reopened = service
        .open(
            &mut guard,
            &id,
            AgentMode::Durable,
            None,
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert!(Arc::ptr_eq(&replacement, &reopened));
    assert_eq!(reopened.read(OplogIndex::INITIAL).await, replacement_entry);
    assert_eq!(
        reopened.add(OplogEntry::no_op(None)).await.unwrap(),
        OplogIndex::from_u64(2)
    );
    reopened.commit(CommitLevel::Always).await.unwrap();
    reopened.stop_and_wait().await.unwrap();
}

#[test]
async fn stopped_actor_failure_does_not_poison_reopen(_tracing: &Tracing) {
    let service = PrimaryOplogService::new(
        Arc::new(InMemoryIndexedStorage::new()),
        Arc::new(InMemoryBlobStorage::new()),
        100,
        100,
        100,
        RetryConfig::default(),
    )
    .await;
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "failed-actor".into(),
    };
    let environment_id = EnvironmentId::new();
    let id = OwnedAgentId::new(environment_id, &agent_id);
    let metadata = make_agent_metadata(agent_id.clone(), AccountId::new(), environment_id);
    let initial = OplogEntry::no_op(None).rounded();
    let mut guard = service.lock_lifecycle(&agent_id).await;
    let old = service
        .create_fresh(
            &mut guard,
            &id,
            AgentMode::Durable,
            initial.clone(),
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert_panics(old.add_pair(
        OplogEntry::no_op(None),
        Box::new(|_| panic!("injected actor failure")),
    ))
    .await;
    assert!(old.stop_and_wait().await.is_err());
    let reopened = service
        .open(
            &mut guard,
            &id,
            AgentMode::Durable,
            None,
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert!(!Arc::ptr_eq(&old, &reopened));
    assert_eq!(reopened.read(OplogIndex::INITIAL).await, initial);
    assert_eq!(
        reopened.add(OplogEntry::no_op(None)).await.unwrap(),
        OplogIndex::from_u64(2)
    );
    reopened.commit(CommitLevel::Always).await.unwrap();
    drop(old);
    reopened.stop_and_wait().await.unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn reopened_oplog_joins_old_root_children_before_stopping_its_writer(_tracing: &Tracing) {
    use crate::worker::tasks::TaskScope;

    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let storage = Arc::new(InMemoryIndexedStorage::new());
        let primary = Arc::new(
            PrimaryOplogService::new(
                storage.clone(),
                Arc::new(InMemoryBlobStorage::new()),
                100,
                100,
                100,
                RetryConfig::default(),
            )
            .await,
        );
        let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
            storage,
            1,
            RetryConfig::default(),
        ));
        let service = MultiLayerOplogService::new(primary, nev![archive], 100, 100);
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "root-join".into(),
        };
        let environment_id = EnvironmentId::new();
        let id = OwnedAgentId::new(environment_id, &agent_id);
        let mut metadata = make_agent_metadata(agent_id.clone(), AccountId::new(), environment_id);
        metadata.agent_mode = mode;
        let mut guard = service.lock_lifecycle(&agent_id).await;
        let old = service
            .create_fresh(
                &mut guard,
                &id,
                mode,
                OplogEntry::no_op(None),
                metadata.clone(),
                default_last_known_status(),
                default_execution_status(mode),
                None,
            )
            .await;
        let reopened = service
            .open(
                &mut guard,
                &id,
                mode,
                None,
                metadata.clone(),
                default_last_known_status(),
                default_execution_status(mode),
                None,
            )
            .await;
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let root = tokio::spawn({
            let old = old.clone();
            async move {
                let scope = TaskScope::default();
                let owner = old.task_owner().expect("production oplog has a task owner");
                scope.bind(owner).unwrap();
                scope
                    .run(async {
                        let writer = old.clone();
                        let job = tokio::spawn(async move {
                            started.send(()).unwrap();
                            release_rx.await.unwrap();
                            assert_eq!(
                                writer.add(OplogEntry::no_op(None)).await.unwrap(),
                                OplogIndex::from_u64(2)
                            );
                            writer.commit(CommitLevel::Always).await.unwrap();
                        });
                        owner.finish_on_drop(job).await.unwrap();
                    })
                    .await
            }
        });
        started_rx.await.unwrap();
        let stop = reopened.stop_and_wait();
        tokio::pin!(stop);
        assert!(futures::poll!(stop.as_mut()).is_pending());
        assert!(root.await.unwrap().is_none());
        assert!(futures::poll!(stop.as_mut()).is_pending());
        release.send(()).unwrap();
        stop.await.unwrap();
        assert_eq!(
            service.get_last_index(&id, mode).await,
            OplogIndex::from_u64(2)
        );
        assert!(
            TaskScope::default()
                .bind(old.task_owner().unwrap())
                .is_err()
        );
        let replacement = service
            .open(
                &mut guard,
                &id,
                mode,
                None,
                metadata,
                default_last_known_status(),
                default_execution_status(mode),
                None,
            )
            .await;
        assert!(
            TaskScope::default()
                .bind(replacement.task_owner().unwrap())
                .is_ok()
        );
        replacement.stop_and_wait().await.unwrap();
    }
}

#[test]
async fn explicit_commit_reports_threshold_commits_once_and_preserves_add_receipts(
    _tracing: &Tracing,
) {
    let service = PrimaryOplogService::new(
        Arc::new(InMemoryIndexedStorage::new()),
        Arc::new(InMemoryBlobStorage::new()),
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "threshold-commit-reporting".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Durable,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            create_entry,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let entries = [
        OplogEntry::suspend().rounded(),
        OplogEntry::exited().rounded(),
        OplogEntry::restart().rounded(),
    ];
    let receipts = entries
        .iter()
        .cloned()
        .map(|entry| oplog.enqueue_add(entry))
        .collect::<Vec<_>>();
    let mut expected = BTreeMap::new();
    for (receipt, entry) in receipts.into_iter().zip(entries) {
        expected.insert(receipt.await.unwrap(), entry);
    }

    assert_eq!(oplog.commit(CommitLevel::Always).await.unwrap(), expected);
    assert!(oplog.commit(CommitLevel::Always).await.unwrap().is_empty());
}

#[test]
async fn archiving_auto_committed_entries_does_not_consume_explicit_commit_report(
    _tracing: &Tracing,
) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage,
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary, nev![archive], 2, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "archive-threshold-commit-reporting".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::NONE,
                    end: OplogIndex::NONE,
                },
            ),
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let entries = [
        OplogEntry::suspend().rounded(),
        OplogEntry::exited().rounded(),
        OplogEntry::restart().rounded(),
    ];
    let mut expected = BTreeMap::new();
    for entry in entries {
        let index = oplog.add(entry.clone()).await.unwrap();
        expected.insert(index, entry);
    }

    MultiLayerOplog::try_archive_blocking(&oplog).await;

    assert_eq!(oplog.commit(CommitLevel::Always).await.unwrap(), expected);
    assert!(oplog.commit(CommitLevel::Always).await.unwrap().is_empty());

    let mut before_commit = BTreeMap::new();
    for entry in [
        OplogEntry::interrupted().rounded(),
        OplogEntry::resumed().rounded(),
    ] {
        before_commit.insert(oplog.add(entry.clone()).await.unwrap(), entry);
    }
    let mut commit = std::pin::pin!(oplog.commit(CommitLevel::Always));
    assert!(futures::poll!(commit.as_mut()).is_pending());

    // The primary processes the queued commit before these adds. Do not poll the outer
    // commit again until the later entries have automatically committed.
    let mut after_commit = BTreeMap::new();
    for entry in [
        OplogEntry::suspend().rounded(),
        OplogEntry::restart().rounded(),
    ] {
        after_commit.insert(oplog.add(entry.clone()).await.unwrap(), entry);
    }
    assert_eq!(commit.await.unwrap(), before_commit);
    MultiLayerOplog::try_archive_blocking(&oplog).await;
    assert_eq!(
        oplog.commit(CommitLevel::Always).await.unwrap(),
        after_commit
    );
    assert!(oplog.commit(CommitLevel::Always).await.unwrap().is_empty());
}

#[test]
async fn wait_for_replicas_does_not_consume_explicit_commit_report(_tracing: &Tracing) {
    let service = PrimaryOplogService::new(
        Arc::new(InMemoryIndexedStorage::new()),
        Arc::new(InMemoryBlobStorage::new()),
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let oplog =
        create_append_reconciliation_oplog(&service, "replica-barrier-commit-reporting").await;
    let entries = [
        OplogEntry::suspend().rounded(),
        OplogEntry::exited().rounded(),
        OplogEntry::restart().rounded(),
    ];
    let mut expected = BTreeMap::new();
    for entry in entries {
        let index = oplog.add(entry.clone()).await.unwrap();
        expected.insert(index, entry);
    }

    assert!(oplog.wait_for_replicas(1, Duration::from_secs(1)).await);

    assert_eq!(oplog.commit(CommitLevel::Always).await.unwrap(), expected);
    assert!(oplog.commit(CommitLevel::Always).await.unwrap().is_empty());
}

#[test]
async fn initial_append_committed_then_indeterminate_is_reconciled(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    indexed_storage.inject_append_failure(InjectedAppendFailure::CommitThenIndeterminate);

    let oplog = create_append_reconciliation_oplog(&service, "reconcile-initial-append").await;

    assert_eq!(oplog.current_oplog_index().await, OplogIndex::INITIAL);
    assert_eq!(indexed_storage.append_attempts(), 1);
    assert_eq!(indexed_storage.reads(), 1);
}

#[test]
async fn retried_append_many_accepts_only_the_same_serialized_batch(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "reconcile-append-many").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.inject_append_many_failure(InjectedAppendFailure::CommitThenIndeterminate);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    oplog.add(OplogEntry::exited()).await.expect("oplog write");
    oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");

    assert_eq!(oplog.current_oplog_index().await, OplogIndex::from_u64(3));
    assert_eq!(indexed_storage.append_many_attempts(), 1);
    assert_eq!(indexed_storage.reads(), 1);
    assert!(indexed_storage.append_many_reused_batch());
}

#[test]
async fn indeterminate_append_before_write_retries_after_empty_read_back(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "uncommitted-append-retry").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.inject_append_many_failure(InjectedAppendFailure::IndeterminateBeforeWrite);

    let entry = OplogEntry::suspend().rounded();
    oplog.add(entry.clone()).await.expect("oplog write");
    oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");

    assert_eq!(indexed_storage.append_many_attempts(), 2);
    assert_eq!(indexed_storage.reads(), 1);
    assert!(indexed_storage.append_many_reused_batch());
    assert_eq!(oplog.read(OplogIndex::from_u64(2)).await, entry);
}

#[test]
async fn exhausted_retries_after_committed_indeterminate_append_reconcile(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "exhausted-append-retries").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.hidden_reads.store(2, Ordering::Relaxed);
    indexed_storage.inject_append_many_failures([
        InjectedAppendFailure::CommitThenIndeterminate,
        InjectedAppendFailure::TransientBeforeWrite,
        InjectedAppendFailure::TransientBeforeWrite,
    ]);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");

    assert_eq!(indexed_storage.append_many_attempts(), 3);
    assert_eq!(indexed_storage.reads(), 3);
}

#[test]
async fn permanent_retry_failure_after_committed_indeterminate_append_reconciles(
    _tracing: &Tracing,
) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "permanent-after-indeterminate").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.hidden_reads.store(1, Ordering::Relaxed);
    indexed_storage.inject_append_many_failures([
        InjectedAppendFailure::CommitThenIndeterminate,
        InjectedAppendFailure::PermanentBeforeWrite,
    ]);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");

    assert_eq!(indexed_storage.append_many_attempts(), 2);
    assert_eq!(indexed_storage.reads(), 2);
    assert_eq!(oplog.current_oplog_index().await, OplogIndex::from_u64(2));
}

#[test]
async fn reconciliation_retries_read_failures_without_resubmitting_append(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "read-failure-reconciliation").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.inject_append_many_failure(InjectedAppendFailure::CommitThenIndeterminate);
    indexed_storage
        .read_failures
        .lock()
        .unwrap()
        .push_back(IndexedStorageError::Transient(
            "connection lost during read-back".to_string(),
        ));

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");

    assert_eq!(indexed_storage.append_many_attempts(), 1);
    assert_eq!(indexed_storage.reads(), 2);
}

#[test]
async fn conflict_after_initially_empty_reconciliation_accepts_exact_batch(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "late-commit-reconciliation").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.inject_append_many_failure(InjectedAppendFailure::CommitThenIndeterminate);
    indexed_storage.hidden_reads.store(1, Ordering::Relaxed);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");

    assert_eq!(indexed_storage.append_many_attempts(), 2);
    assert_eq!(indexed_storage.reads(), 2);
    assert!(indexed_storage.append_many_reused_batch());
}

#[test]
async fn direct_identical_append_conflict_from_second_writer_remains_fatal(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let first_service = append_reconciliation_service(indexed_storage.clone()).await;
    let second_service = append_reconciliation_service(indexed_storage.clone()).await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "concurrent-identical-append".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let first_oplog = first_service
        .create_fresh(
            &mut first_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::NONE,
                    end: OplogIndex::NONE,
                },
            ),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let second_oplog = second_service
        .open(
            &mut second_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::INITIAL),
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let entry = OplogEntry::suspend();
    first_oplog.add(entry.clone()).await.expect("oplog write");
    second_oplog.add(entry).await.expect("oplog write");
    first_oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");
    indexed_storage.reset();
    indexed_storage.reset_append_observations();

    assert_panics(second_oplog.commit(CommitLevel::Always)).await;

    assert_eq!(indexed_storage.append_many_attempts(), 1);
    assert_eq!(indexed_storage.reads(), 0);
}

#[test]
async fn incomplete_read_back_after_indeterminate_append_remains_fatal(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "incomplete-append-read-back").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage
        .inject_append_many_failure(InjectedAppendFailure::CommitPrefixThenIndeterminate);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    oplog.add(OplogEntry::exited()).await.expect("oplog write");
    assert_panics(oplog.commit(CommitLevel::Always)).await;

    assert_eq!(indexed_storage.append_many_attempts(), 1);
    assert_eq!(indexed_storage.reads(), 1);
}

#[test]
async fn differing_read_back_after_indeterminate_append_remains_fatal(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog(&service, "different-append-read-back").await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage
        .inject_append_many_failure(InjectedAppendFailure::CommitDifferentThenIndeterminate);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    assert_panics(oplog.commit(CommitLevel::Always)).await;

    assert_eq!(indexed_storage.append_many_attempts(), 1);
    assert_eq!(indexed_storage.reads(), 1);
}

/// Same mismatch as `differing_read_back_after_indeterminate_append_remains_fatal`, except this
/// writer asserts a shard epoch and the mismatch is explained: a new owner already wrote those
/// indices. The reconciliation probe this fences must return `Fenced` and let the caller
/// give up the agent, rather than panicking and aborting the whole - otherwise still live -
/// executor process (`panic = "abort"`).
#[test]
async fn differing_read_back_on_a_moved_shard_is_fenced_instead_of_panicking(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let service = append_reconciliation_service(indexed_storage.clone()).await;
    let oplog = create_append_reconciliation_oplog_with_epoch(
        &service,
        "different-append-read-back-fenced",
        Some(ShardEpoch(5)),
    )
    .await;
    indexed_storage.reset();
    indexed_storage.reset_append_observations();
    indexed_storage.inject_append_many_failures([
        InjectedAppendFailure::CommitDifferentThenIndeterminate,
        InjectedAppendFailure::Fenced,
    ]);

    oplog.add(OplogEntry::suspend()).await.expect("oplog write");
    let result = oplog.commit(CommitLevel::Always).await;

    assert!(
        matches!(result, Err(OplogError::Fenced(_))),
        "expected the reconciliation probe to surface a fence instead of panicking, got {result:?}"
    );
    // The original attempt, then the reconciliation probe once the read-back mismatched.
    assert_eq!(indexed_storage.append_many_attempts(), 2);
}

#[test]
async fn a_delete_that_outlived_the_shard_leaves_the_oplog_to_its_new_owner(_tracing: &Tracing) {
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
    let archive = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage.clone(),
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary.clone(), nev![archive], 100, 1);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "delete-after-the-shard-moved".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = service
        .create(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::no_op(None),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(5)),
        )
        .await;
    oplog.add_and_commit(OplogEntry::no_op(None)).await.unwrap();
    let last = oplog.current_oplog_index().await;
    drop(oplog);

    // The shard moves on and another executor takes the oplog over at the next epoch, while this
    // one is still working through a deletion it accepted at epoch 5.
    indexed_storage
        .for_writer(WriterId(Uuid::new_v4()))
        .set_key_epoch(
            "oplog",
            "set_key_epoch",
            IndexedStorageNamespace::OpLog {
                agent_id: agent_id.clone(),
                agent_mode: AgentMode::Durable,
            },
            &agent_id.to_redis_key(),
            ShardEpoch(6),
        )
        .await
        .unwrap();

    let result = service
        .delete(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(ShardEpoch(5)),
        )
        .await;

    match result {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.expected_epoch, ShardEpoch(5));
            assert_eq!(fence.actual_epoch, Some(ShardEpoch(6)));
        }
        other => panic!("expected the delete to be fenced, got {other:?}"),
    }
    assert!(
        service.exists(&owned_agent_id, AgentMode::Durable).await,
        "a refused delete removes nothing"
    );
    assert_eq!(
        primary
            .get_last_index(&owned_agent_id, AgentMode::Durable)
            .await,
        last
    );
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let entry1 = OplogEntry::jump(
        None,
        OplogRegion {
            start: OplogIndex::from_u64(5),
            end: OplogIndex::from_u64(12),
        },
    )
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();

    let last_oplog_idx = oplog.current_oplog_index().await;
    oplog.add(entry1.clone()).await.unwrap();
    oplog.add(entry2.clone()).await.unwrap();
    oplog.add(entry3.clone()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);

    let entries = oplog_service
        .read_exact(
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
    assert_panics(oplog.read_exact(last_oplog_idx.next(), 4)).await;
}

#[test]
async fn primary_read_range_overflow_panics_without_storage_io(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage.clone(),
        Arc::new(InMemoryBlobStorage::new()),
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "overflow".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let start = OplogIndex::from_u64(u64::MAX);

    assert_panics(oplog_service.read_exact(&owned_agent_id, AgentMode::Durable, start, 2)).await;

    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::NONE),
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert_panics(oplog.read_exact(start, 2)).await;
    assert_eq!(indexed_storage.reads(), 0);
}

#[test]
async fn primary_storage_read_failures_panic_from_all_read_paths(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::failing_reads(
        IndexedStorageError::Other("injected permanent read failure".to_string()),
    ));
    let oplog_service = PrimaryOplogService::new(
        indexed_storage.clone(),
        Arc::new(InMemoryBlobStorage::new()),
        1,
        1,
        100,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "failed-read".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    assert_panics(oplog_service.read_exact(
        &owned_agent_id,
        AgentMode::Durable,
        OplogIndex::INITIAL,
        1,
    ))
    .await;

    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::INITIAL),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert_panics(oplog.read_exact(OplogIndex::INITIAL, 1)).await;
    drop(oplog);

    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::INITIAL),
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert_panics(oplog.read(OplogIndex::INITIAL)).await;
    assert_eq!(indexed_storage.reads(), 3);
}

#[test]
async fn exhausted_primary_read_retries_panic(_tracing: &Tracing) {
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::failing_reads(
        IndexedStorageError::Transient("injected transient read failure".to_string()),
    ));
    let retry_config = RetryConfig {
        max_attempts: 3,
        min_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        multiplier: 1.0,
        max_jitter_factor: None,
    };
    let oplog_service = PrimaryOplogService::new(
        indexed_storage.clone(),
        Arc::new(InMemoryBlobStorage::new()),
        1,
        1,
        100,
        retry_config,
    )
    .await;
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "exhausted-read-retries".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);

    assert_panics(oplog_service.read_exact(
        &owned_agent_id,
        AgentMode::Durable,
        OplogIndex::INITIAL,
        1,
    ))
    .await;
    assert_eq!(indexed_storage.reads(), 3);
}

#[test]
async fn durable_stream_batch_uses_payload_threshold_for_each_record(_tracing: &Tracing) {
    use golem_common::base_model::durable_stream::{
        LocalStreamId, StreamCancelReason, StreamCancelRecord, StreamCancelRole, StreamEndRecord,
        StreamEndResult, StreamInvocationId, StreamItemsPayload, StreamItemsRecord, StreamOffset,
        StreamRegisteredRecord, StreamRegistrationInvocation, StreamRegistrationRecordCoordinate,
        StreamRootKind, StreamSessionFinishedRecord, StreamSessionRecord, StreamSourceKind,
        StreamTerminalAuthor,
    };
    use golem_common::model::component::ComponentRevision;
    use golem_schema::schema::SchemaFingerprintV1;

    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        100,
        100,
        1000,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "stream-payload".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let producer_fingerprint = AgentFingerprint(Uuid::new_v4());
    let invocation_id = StreamInvocationId {
        callee_environment_id: environment_id,
        callee: agent_id.clone(),
        callee_fingerprint: producer_fingerprint,
        idempotency_key: IdempotencyKey::new("stream-invocation".to_string()),
    };
    let added = oplog
        .add_durable_stream_batch(Box::new(move |registration_index| {
            let stream_id = LocalStreamId(registration_index);
            let item_index = registration_index.next();
            let end_index = item_index.next();
            let cancel_index = end_index.next();
            vec![
                DurableStreamOplogRecord::Registered(
                    None,
                    Box::new(StreamRegisteredRecord {
                        format_version: 1,
                        coordinate: StreamRegistrationRecordCoordinate::Root {
                            invocation: StreamRegistrationInvocation::Local(
                                invocation_id.idempotency_key.clone(),
                            ),
                            root_kind: StreamRootKind::MethodResult,
                            recursive_value_path: Vec::new(),
                        },
                        source_invocation: StreamRegistrationInvocation::Local(
                            invocation_id.idempotency_key.clone(),
                        ),
                        component_revision: ComponentRevision::INITIAL,
                        element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                        source_kind: StreamSourceKind::InvocationOutput,
                        session_role: None,
                    }),
                ),
                DurableStreamOplogRecord::Items(
                    None,
                    StreamItemsRecord {
                        format_version: 1,
                        stream_id,
                        first_sequence: 0,
                        nested_stream_ids: Vec::new(),
                        newly_registered_stream_ids: Vec::new(),
                        payload: StreamItemsPayload::Values(vec![vec![42; 1024]]),
                        offsets: vec![StreamOffset::new(item_index, 0)],
                    },
                ),
                DurableStreamOplogRecord::End(
                    None,
                    StreamEndRecord {
                        format_version: 1,
                        stream_id,
                        sequence: 1,
                        offset: StreamOffset::new(end_index, 0),
                        authored_by: StreamTerminalAuthor::Guest,
                        result: StreamEndResult::Ok,
                    },
                ),
                DurableStreamOplogRecord::Cancel(
                    None,
                    StreamCancelRecord {
                        format_version: 1,
                        stream_id,
                        sequence: 1,
                        offset: StreamOffset::new(cancel_index, 0),
                        authored_by: StreamTerminalAuthor::Protocol,
                        role: StreamCancelRole::OutputConsumer,
                        reason: StreamCancelReason::Protocol,
                        details: Some("test cancellation".to_string()),
                    },
                ),
                DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                        format_version: 1,
                        session_key: golem_common::model::durable_stream::StreamRegistrationInvocation::Local(
                            invocation_id.idempotency_key,
                        ),
                        result: Err(vec![57; 1024]),
                    })),
                ),
            ]
        }))
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    assert_eq!(added.len(), 5);
    for (_, entry) in added {
        match entry {
            OplogEntry::StreamRegistered { record, .. } => {
                assert!(matches!(&record, OplogPayload::SerializedInline { .. }));
                oplog.download_payload(record).await.unwrap();
            }
            OplogEntry::StreamItems { record, .. } => {
                assert!(matches!(
                    &record,
                    OplogPayload::External {
                        cached: Some(_),
                        ..
                    }
                ));
                let record = oplog.download_payload(record).await.unwrap();
                assert_eq!(
                    record.payload,
                    StreamItemsPayload::Values(vec![vec![42; 1024]])
                );
            }
            OplogEntry::StreamEnd { record, .. } => {
                assert!(matches!(&record, OplogPayload::SerializedInline { .. }));
                oplog.download_payload(record).await.unwrap();
            }
            OplogEntry::StreamCancel { record, .. } => {
                assert!(matches!(&record, OplogPayload::SerializedInline { .. }));
                oplog.download_payload(record).await.unwrap();
            }
            OplogEntry::StreamSession { record, .. } => {
                assert!(matches!(
                    &record,
                    OplogPayload::External {
                        cached: Some(_),
                        ..
                    }
                ));
                let StreamSessionRecord::Finished(record) =
                    oplog.download_payload(record).await.unwrap()
                else {
                    panic!("expected finished session");
                };
                assert_eq!(record.result, Err(vec![57; 1024]));
            }
            _ => panic!("durable stream batch appended a non-stream entry"),
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn ephemeral_durable_stream_batch_keeps_terminals_inline_atomically(_tracing: &Tracing) {
    use golem_common::base_model::durable_stream::{
        LocalStreamId, StreamEndRecord, StreamEndResult, StreamInvocationId, StreamOffset,
        StreamSessionFinishedRecord, StreamSessionRecord, StreamTerminalAuthor,
    };
    use golem_common::model::component::ComponentRevision;

    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage,
            100,
            0,
            usize::MAX,
            RetryConfig::default(),
        )
        .await,
    );
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage,
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary, nev![archive], 100, 0);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "ephemeral-stream-batch".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let mut metadata = make_agent_metadata(agent_id.clone(), account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = service
        .create(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_test_entry(
                agent_id.clone(),
                AgentMode::Ephemeral,
                ComponentRevision::INITIAL,
                environment_id,
                account_id,
                Uuid::new_v4(),
            )
            .rounded(),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;
    let session_key = StreamInvocationId {
        callee_environment_id: environment_id,
        callee: agent_id,
        callee_fingerprint: AgentFingerprint(Uuid::new_v4()),
        idempotency_key: IdempotencyKey::new("ephemeral-terminal-session".to_string()),
    };

    let added = oplog
        .add_durable_stream_batch(Box::new(move |first_index| {
            vec![
                DurableStreamOplogRecord::End(
                    None,
                    StreamEndRecord {
                        format_version: 1,
                        stream_id: LocalStreamId(first_index),
                        sequence: 0,
                        offset: StreamOffset::new(first_index, 0),
                        authored_by: StreamTerminalAuthor::Guest,
                        result: StreamEndResult::Ok,
                    },
                ),
                DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                        format_version: 1,
                        session_key: golem_common::model::durable_stream::StreamRegistrationInvocation::Local(session_key.idempotency_key),
                        result: Err(vec![91; 1024]),
                    })),
                ),
            ]
        }))
        .await
        .unwrap();

    assert_eq!(added.len(), 2);
    assert_eq!(added[1].0, added[0].0.next());
    assert_eq!(oplog.current_oplog_index().await, OplogIndex::from_u64(3));
    let resident = oplog.read_exact(added[0].0, 2).await;
    assert_eq!(resident, added.iter().cloned().collect());
    // Threshold handoff is asynchronous; protocol publication uses an explicit barrier.
    oplog.commit(CommitLevel::Always).await.unwrap();
    let persisted = service
        .read_exact(
            &owned_agent_id,
            AgentMode::Ephemeral,
            OplogIndex::INITIAL,
            3,
        )
        .await;
    assert_eq!(persisted.len(), 3);
    assert!(matches!(
        persisted[&OplogIndex::INITIAL],
        OplogEntry::Create { .. }
    ));
    assert!(matches!(
        persisted[&added[0].0],
        OplogEntry::StreamEnd { .. }
    ));
    let OplogEntry::StreamSession { record, .. } = &persisted[&added[1].0] else {
        panic!("persisted terminal batch must end with a session record");
    };
    let StreamSessionRecord::Finished(finished) =
        oplog.download_payload(record.clone()).await.unwrap()
    else {
        panic!("persisted terminal batch must end with Finished");
    };
    assert_eq!(finished.result, Err(vec![91; 1024]));
    assert!(matches!(
        &added[0].1,
        OplogEntry::StreamEnd {
            record: OplogPayload::Inline(_),
            ..
        }
    ));
    let session_payload = match &added[1].1 {
        OplogEntry::StreamSession {
            record: payload @ OplogPayload::Inline(_),
            ..
        } => payload.clone(),
        other => panic!("expected inline Finished session, got {other:?}"),
    };
    let StreamSessionRecord::Finished(finished) =
        oplog.download_payload(session_payload).await.unwrap()
    else {
        panic!("expected finished session");
    };
    assert_eq!(finished.result, Err(vec![91; 1024]));
}

#[test]
#[test_r::timeout("30s")]
async fn blocked_durable_stream_batch_prepares_before_atomic_commit_and_append(_tracing: &Tracing) {
    use golem_common::base_model::durable_stream::{
        LocalStreamId, StreamEndRecord, StreamEndResult, StreamInvocationId, StreamOffset,
        StreamSessionFinishedRecord, StreamSessionRecord, StreamTerminalAuthor,
    };

    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(ReadCountingBlobStorage::new());
    let (put_started, release_put) = blob_storage.pause_next_put();
    let service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            0,
            0,
            8,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "blocked-stream-batch".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let generated = Arc::new(AtomicUsize::new(0));
    let produced = generated.clone();
    let session_key = StreamInvocationId {
        callee_environment_id: environment_id,
        callee: owned_agent_id.agent_id.clone(),
        callee_fingerprint: AgentFingerprint(Uuid::new_v4()),
        idempotency_key: IdempotencyKey::new("blocked-terminal-session".to_string()),
    };
    let batch_oplog = oplog.clone();
    let batch = tokio::spawn(async move {
        batch_oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let finished_produced = produced.clone();
                let terminals = (0..2).map(move |position| {
                    produced.fetch_add(1, Ordering::SeqCst);
                    DurableStreamOplogRecord::End(
                        None,
                        StreamEndRecord {
                            format_version: 1,
                            stream_id: LocalStreamId(OplogIndex::from_u64(
                                first_index.as_u64() + position,
                            )),
                            sequence: position,
                            offset: StreamOffset::new(
                                OplogIndex::from_u64(first_index.as_u64() + position),
                                0,
                            ),
                            authored_by: StreamTerminalAuthor::Guest,
                            result: StreamEndResult::Ok,
                        },
                    )
                });
                terminals
                    .chain(std::iter::once_with(move || {
                        finished_produced.fetch_add(1, Ordering::SeqCst);
                        DurableStreamOplogRecord::Session(
                            None,
                            Box::new(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                                format_version: 1,
                                session_key: golem_common::model::durable_stream::StreamRegistrationInvocation::Local(
                                    session_key.idempotency_key,
                                ),
                                result: Err(vec![73; 1024]),
                            })),
                        )
                    }))
                    .collect()
            }))
            .await
    });
    put_started.await.unwrap();

    assert_eq!(generated.load(Ordering::SeqCst), 3);
    assert!(
        service
            .read_source(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 3)
            .await
            .is_empty(),
        "the blocked first upload must not expose a partial reference"
    );
    let competing_commit = oplog.commit(CommitLevel::Always);
    tokio::pin!(competing_commit);
    assert!(futures::poll!(&mut competing_commit).is_pending());
    let competing_append = oplog.add(OplogEntry::suspend().rounded());
    tokio::pin!(competing_append);
    assert!(futures::poll!(&mut competing_append).is_pending());

    release_put.send(()).unwrap();
    let added = batch.await.unwrap().unwrap();
    competing_commit.await.unwrap();
    let competing_index = competing_append.await.unwrap();

    assert_eq!(generated.load(Ordering::SeqCst), 3);
    assert_eq!(added.len(), 3);
    assert_eq!(added[0].0, OplogIndex::INITIAL);
    assert_eq!(added[1].0, added[0].0.next());
    assert_eq!(added[2].0, added[1].0.next());
    assert_eq!(competing_index, added[2].0.next());
    assert!(matches!(
        &added[2].1,
        OplogEntry::StreamSession {
            record: OplogPayload::External {
                cached: Some(_),
                ..
            },
            ..
        }
    ));
    let committed = service
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 4)
        .await;
    assert_eq!(committed.len(), 4);
    assert!(matches!(
        committed[&competing_index],
        OplogEntry::Suspend { .. }
    ));
}

#[test]
async fn durable_stream_producer_recovers_from_sqlite_storage_restart(_tracing: &Tracing) {
    use crate::durable_host::durable_stream::{
        CommittedProducerStreamEventPayload, DurableStreamStore, ProducerRegistrationRequest,
    };
    use golem_common::base_model::durable_stream::{
        StreamEndResult, StreamInvocationId, StreamItemsPayload, StreamRegistrationCoordinate,
        StreamRegistrationInvocation, StreamRootKind, StreamSourceKind,
    };
    use golem_common::model::component::ComponentRevision;
    use golem_schema::schema::SchemaFingerprintV1;

    let tempdir = tempfile::TempDir::new().expect("Cannot create temp dir");
    let config = golem_common::config::DbSqliteConfig {
        database: tempdir
            .path()
            .join("durable-stream.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 4,
        foreign_keys: false,
    };
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "durable-stream-restart".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let producer_fingerprint = AgentFingerprint(Uuid::new_v4());
    let invocation_id = StreamInvocationId {
        callee_environment_id: environment_id,
        callee: agent_id.clone(),
        callee_fingerprint: producer_fingerprint,
        idempotency_key: IdempotencyKey::new("durable-stream-invocation".to_string()),
    };
    let registration = ProducerRegistrationRequest {
        coordinate: StreamRegistrationCoordinate::Root {
            invocation_id: invocation_id.clone(),
            root_kind: StreamRootKind::MethodResult,
            recursive_value_path: Vec::new(),
        },
        source_invocation: StreamRegistrationInvocation::Local(invocation_id.idempotency_key),
        component_revision: ComponentRevision::INITIAL,
        element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
        source_kind: StreamSourceKind::InvocationOutput,
        session_mapping: None,
        entity_parent_start_index: None,
    };

    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> =
        Arc::new(SqliteIndexedStorage::configured(&config).await.unwrap());
    let service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage.clone(),
        100,
        100,
        128,
        RetryConfig::default(),
    )
    .await;
    let oplog = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let producer = DurableStreamStore::load(
        oplog,
        environment_id,
        agent_id.clone(),
        producer_fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(None, registration.clone())
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
        .end(None, handle.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap();
    drop(producer);
    drop(service);

    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> =
        Arc::new(SqliteIndexedStorage::configured(&config).await.unwrap());
    let restarted_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        100,
        100,
        128,
        RetryConfig::default(),
    )
    .await;
    let restarted_oplog = restarted_service
        .open(
            &mut restarted_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let restarted = DurableStreamStore::load(
        restarted_oplog.clone(),
        environment_id,
        agent_id,
        producer_fingerprint,
        None,
    )
    .await
    .unwrap();
    assert!(
        restarted
            .register(None, registration)
            .await
            .unwrap()
            .replayed
    );
    assert!(
        restarted
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![42]]),
            )
            .await
            .unwrap()
            .replayed
    );
    assert!(
        restarted
            .end(None, handle.stream_id, 1, StreamEndResult::Ok)
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(
        restarted_oplog.current_oplog_index().await,
        OplogIndex::from_u64(3)
    );

    let mut reader = restarted.catch_up(handle, None).await.unwrap();
    assert_eq!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::Value(vec![42])
    );
    assert_eq!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    );
    assert!(reader.next().await.unwrap().is_none());
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let entry1 = OplogEntry::jump(
        None,
        OplogRegion {
            start: OplogIndex::from_u64(5),
            end: OplogIndex::from_u64(12),
        },
    )
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();
    let entry4 = OplogEntry::interrupted().rounded();
    let entry5 = OplogEntry::no_op(None).rounded();

    oplog.add(entry1.clone()).await.unwrap();
    oplog.add(entry2.clone()).await.unwrap();
    oplog.add(entry3.clone()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    oplog.add(entry4.clone()).await.unwrap();
    oplog.add(entry5.clone()).await.unwrap(); // uncommitted entries

    let read_count = indexed_storage.read_count();
    let buffered_entries = oplog
        .read_exact(OplogIndex::from_u64(4), 2)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(buffered_entries, vec![entry4.clone(), entry5.clone()]);
    assert_eq!(indexed_storage.read_count(), read_count);

    assert_eq!(oplog.read(OplogIndex::from_u64(4)).await, entry4.clone());
    assert_eq!(oplog.read(OplogIndex::from_u64(5)).await, entry5.clone());
    assert_eq!(indexed_storage.read_count(), read_count);

    let entries = oplog
        .read_exact(OplogIndex::INITIAL, 5)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(
        entries,
        vec![entry1, entry2, entry3, entry4, entry5.clone()]
    );
    assert_eq!(indexed_storage.read_count(), read_count + 1);

    let entry = oplog
        .read_exact(OplogIndex::from_u64(5), 1)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entry, vec![entry5]);
    assert_eq!(indexed_storage.read_count(), read_count + 1);

    let read_count = indexed_storage.read_count();
    assert!(
        oplog
            .read_exact(OplogIndex::from_u64(5), 0)
            .await
            .is_empty()
    );
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    let entry1 = OplogEntry::jump(
        None,
        OplogRegion {
            start: OplogIndex::from_u64(5),
            end: OplogIndex::from_u64(12),
        },
    )
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();

    let last_oplog_idx = oplog.current_oplog_index().await;
    oplog.add(entry1.clone()).await.unwrap();
    oplog.add(entry2.clone()).await.unwrap();
    oplog.add(entry3.clone()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);

    let entries = oplog_service
        .read_exact(
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
    assert_panics(oplog_service.read_exact(
        &owned_agent_id,
        AgentMode::Durable,
        last_oplog_idx.next(),
        4,
    ))
    .await;
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    let entry1 = OplogEntry::jump(
        None,
        OplogRegion {
            start: OplogIndex::from_u64(5),
            end: OplogIndex::from_u64(12),
        },
    )
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();
    let entry4 = OplogEntry::interrupted().rounded();

    oplog.add(entry1.clone()).await.unwrap();
    oplog.add(entry2.clone()).await.unwrap();
    oplog.add(entry3.clone()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    oplog.add(entry4.clone()).await.unwrap(); // uncommitted

    let entries = oplog
        .read_exact(OplogIndex::INITIAL, 4)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2, entry3, entry4]);
}

#[test]
async fn ephemeral_read_exact_committed_only(_tracing: &Tracing) {
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    let entry1 = OplogEntry::suspend().rounded();
    let entry2 = OplogEntry::exited().rounded();
    let entry3 = OplogEntry::interrupted().rounded();

    oplog.add(entry1.clone()).await.unwrap();
    oplog.add(entry2.clone()).await.unwrap();
    oplog.add(entry3.clone()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    // All committed, no buffer entries
    let entries = oplog
        .read_exact(OplogIndex::INITIAL, 3)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2, entry3]);
}

#[test]
async fn ephemeral_read_exact_uncommitted_only(_tracing: &Tracing) {
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    let entry1 = OplogEntry::suspend().rounded();
    let entry2 = OplogEntry::exited().rounded();

    oplog.add(entry1.clone()).await.unwrap();
    oplog.add(entry2.clone()).await.unwrap();
    // No commit — entries only in the buffer

    let entries = oplog
        .read_exact(OplogIndex::INITIAL, 2)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2]);
}

#[test]
async fn ephemeral_read_exact_partial_range(_tracing: &Tracing) {
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries = Vec::new();
    for i in 0..10 {
        let entry = OplogEntry::Error {
            timestamp,
            entity_parent_start_index: None,
            kind: OplogErrorKind::Invocation,
            error: AgentError::Unknown(i.to_string()),
            retry_from: OplogIndex::NONE,
            inside_atomic_region: false,
            retry_policy_state: None,
        }
        .rounded();
        oplog.add(entry.clone()).await.unwrap();
        entries.push(entry);
    }
    oplog.commit(CommitLevel::Always).await.unwrap();

    // Add 2 more uncommitted
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await.unwrap();
    oplog.add(uncommitted2.clone()).await.unwrap();
    entries.push(uncommitted1);
    entries.push(uncommitted2);

    // Read a sub-range from the middle spanning committed and uncommitted
    let mid_entries = oplog
        .read_exact(OplogIndex::from_u64(8), 4)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(mid_entries, entries[7..11].to_vec());

    // Read just the first 3
    let first3 = oplog
        .read_exact(OplogIndex::INITIAL, 3)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(first3, entries[0..3].to_vec());

    // Read the last 2 (uncommitted only)
    let last2 = oplog
        .read_exact(OplogIndex::from_u64(11), 2)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(last2, entries[10..12].to_vec());

    // Read all
    let all = oplog
        .read_exact(OplogIndex::INITIAL, 12)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(all, entries);
}

#[test]
async fn ephemeral_read_exact_across_archive_layers(_tracing: &Tracing) {
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
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
        oplog.add(entry.clone()).await.unwrap();
    }
    oplog.commit(CommitLevel::Always).await.unwrap();

    // Add 2 uncommitted entries
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await.unwrap();
    oplog.add(uncommitted2.clone()).await.unwrap();
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
        .read_exact(initial_oplog_idx.next(), 10)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(first10, entries[..10].to_vec());

    // Read last 10 — includes uncommitted entries from buffer
    let last10 = oplog
        .read_exact(oplog.current_oplog_index().await.subtract(10).next(), 10)
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
        .read_exact(initial_oplog_idx.next(), entries.len() as u64)
        .await
        .into_values()
        .collect::<Vec<_>>();
    assert_eq!(all, entries);
}

#[test]
async fn ephemeral_read_exact_zero_returns_empty(_tracing: &Tracing) {
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;

    oplog.add(OplogEntry::suspend().rounded()).await.unwrap();

    let entries = oplog.read_exact(OplogIndex::INITIAL, 0).await;
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
        .add_agent_invocation_started(
            AgentInvocation::AgentMethod {
                idempotency_key: IdempotencyKey::fresh(),
                method_name: "f2".to_string(),
                input: SchemaValue::Record {
                    fields: vec![SchemaValue::String("request".to_string())],
                },
                invocation_context: InvocationContextStack::fresh_rounded(),
                principal: Principal::anonymous(),
                scope_card: None,
            },
            invocation_wallet_pin(),
        )
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
        .unwrap();
    let entry3 = oplog.read(entry3).await.rounded();

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
    oplog.add(entry4.clone()).await.unwrap();

    oplog.commit(CommitLevel::Always).await.unwrap();

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
        .read_exact(
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
async fn completed_host_call_response_upload_failure_writes_no_start(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(ReadCountingBlobStorage::failing_on_put(2));
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
        agent_id: "completed-host-call-upload-failure".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let before = oplog.current_oplog_index().await;

    let result = oplog
        .add_completed_host_call(
            HostFunctionName::Custom("completed-call".to_string()),
            &HostRequest::Custom(vec![1u8; 1024].into_typed_schema_value().unwrap()),
            &HostResponse::Custom(vec![2u8; 1024].into_typed_schema_value().unwrap()),
            DurableFunctionType::WriteRemoteBatched(Some(OplogIndex::INITIAL)),
            Some(OplogIndex::INITIAL),
        )
        .await;

    assert!(result.is_err());
    assert_eq!(oplog.current_oplog_index().await, before);
    assert_panics(oplog_service.read_exact(&owned_agent_id, AgentMode::Durable, before.next(), 10))
        .await;

    let parent = OplogIndex::INITIAL;
    let function_name = HostFunctionName::Custom("completed-call".to_string());
    let (start_index, end_index) = oplog
        .add_completed_host_call(
            function_name.clone(),
            &HostRequest::Custom(vec![1u8; 1024].into_typed_schema_value().unwrap()),
            &HostResponse::Custom(vec![2u8; 1024].into_typed_schema_value().unwrap()),
            DurableFunctionType::WriteRemoteBatched(Some(parent)),
            Some(parent),
        )
        .await
        .unwrap();

    assert_eq!(start_index, before.next());
    assert_eq!(end_index, start_index.next());
    assert!(matches!(
        oplog.read(start_index).await,
        OplogEntry::Start {
            parent_start_index: Some(entry_parent),
            function_name: entry_function_name,
            ..
        } if entry_parent == parent && entry_function_name == function_name
    ));
    assert!(matches!(
        oplog.read(end_index).await,
        OplogEntry::End {
            start_index: entry_start_index,
            ..
        } if entry_start_index == start_index
    ));
}

#[test]
async fn owned_invocation_payload_upload_failure_writes_no_entry(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(ReadCountingBlobStorage::failing_on_put(1));
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
        agent_id: "owned-invocation-upload-failure".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let before = oplog.current_oplog_index().await;

    let result = oplog
        .add_agent_invocation_started_with_index(
            AgentInvocation::AgentMethod {
                idempotency_key: IdempotencyKey::fresh(),
                method_name: "large-input".to_string(),
                input: SchemaValue::Binary(BinaryValuePayload {
                    bytes: vec![1_u8; 1024],
                    mime_type: None,
                }),
                invocation_context: InvocationContextStack::fresh_rounded(),
                principal: Principal::anonymous(),
                scope_card: None,
            },
            invocation_wallet_pin(),
        )
        .await;

    assert!(result.is_err());
    assert_eq!(oplog.current_oplog_index().await, before);
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
        .add_agent_invocation_started(
            AgentInvocation::AgentMethod {
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
                scope_card: None,
            },
            invocation_wallet_pin(),
        )
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
        .unwrap();
    let entry3 = oplog.read(entry3).await.rounded();

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
    oplog.add(entry4.clone()).await.unwrap();

    oplog.commit(CommitLevel::Always).await.unwrap();

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
        .read_exact(
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
            invocation_id: None,
            observational_owner: None,
            request: Some(request),
            durable_function_type: DurableFunctionType::ReadLocal,
        }
        .rounded();
        oplog.add(entry.clone()).await.unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();
        entries.push(entry);
    }

    let start = Instant::now();
    loop {
        let primary_length = primary_oplog_service
            .open(
                &mut primary_oplog_service
                    .lock_lifecycle(&owned_agent_id.agent_id)
                    .await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
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
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, n)
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
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
        oplog.add(entry.clone()).await.unwrap();
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await.unwrap();
    oplog.add(uncommitted2.clone()).await.unwrap();

    entries.push(uncommitted1);
    entries.push(uncommitted2);

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
        .read_exact(
            &owned_agent_id,
            AgentMode::Durable,
            initial_oplog_idx.next(),
            10,
        )
        .await;
    let original_first10 = entries.iter().take(10).cloned().collect::<Vec<_>>();

    assert_eq!(first10.into_values().collect::<Vec<_>>(), original_first10);

    let last10 = oplog
        .read_exact(oplog.current_oplog_index().await.subtract(10).next(), 10)
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
    let secondary_inner: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
            RetryConfig::default(),
        ))
    };
    let tertiary_inner: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
            RetryConfig::default(),
        ))
    };
    let secondary_calls = Arc::new(ArchiveCallCounts::default());
    let tertiary_calls = Arc::new(ArchiveCallCounts::default());
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(RecordingArchiveService {
        inner: secondary_inner,
        calls: secondary_calls.clone(),
    });
    let tertiary_layer: Arc<dyn OplogArchiveService> = Arc::new(RecordingArchiveService {
        inner: tertiary_inner,
        calls: tertiary_calls.clone(),
    });
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
        parameters: Box::new(golem_common::model::oplog::CreateParameters {
            owner_kind: OwnerKind::ComponentAgent,
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
        }),
    }
    .rounded();

    let oplog = oplog_service
        .create(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            create_entry.clone(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    // The create entry is in the primary oplog now
    let read1 = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_1 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;
    assert_eq!(secondary_calls.read.load(Ordering::Relaxed), 0);
    assert_eq!(tertiary_calls.read.load(Ordering::Relaxed), 0);

    // Archiving it to the secondary
    let more = MultiLayerOplog::try_archive_blocking(&oplog).await;

    // Reading it again, now it needs to be fetched from the secondary layer
    let read2 = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_2 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;
    assert_eq!(secondary_calls.read.load(Ordering::Relaxed), 1);
    assert_eq!(tertiary_calls.read.load(Ordering::Relaxed), 0);

    // Archiving it to the tertiary
    MultiLayerOplog::try_archive_blocking(&oplog).await;

    // Reading it again, now it needs to be fetched from the tertiary layer
    let read3 = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_3 = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;
    assert_eq!(secondary_calls.read.load(Ordering::Relaxed), 2);
    assert_eq!(tertiary_calls.read.load(Ordering::Relaxed), 1);

    assert_eq!(more, Some(true));
    assert_eq!(read1, Some((OplogIndex::INITIAL, create_entry.clone())));
    assert_eq!(read2, Some((OplogIndex::INITIAL, create_entry.clone())));
    assert_eq!(read3, Some((OplogIndex::INITIAL, create_entry)));

    assert_eq!(last_index_1, OplogIndex::INITIAL);
    assert_eq!(last_index_2, OplogIndex::INITIAL);
    assert_eq!(last_index_3, OplogIndex::INITIAL);

    // With every movable layer empty there is no transfer to wait for
    let nothing_left = tokio::time::timeout(
        Duration::from_secs(10),
        MultiLayerOplog::try_archive_blocking(&oplog),
    )
    .await;
    assert_eq!(nothing_left, Ok(Some(false)));
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
        parameters: Box::new(golem_common::model::oplog::CreateParameters {
            owner_kind: OwnerKind::ComponentAgent,
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
        }),
    }
    .rounded();

    let oplog = oplog_service
        .create(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry.clone(),
            AgentMetadata {
                agent_mode: AgentMode::Ephemeral,
                ..make_agent_metadata(agent_id.clone(), account_id, environment_id)
            },
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;
    oplog.commit(CommitLevel::Always).await.unwrap();

    let read_before_archive = oplog_service
        .read_exact(
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
        .read_exact(
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

    // With every movable layer empty there is no transfer to wait for
    let nothing_left = tokio::time::timeout(
        Duration::from_secs(10),
        EphemeralOplog::try_archive_blocking(&oplog),
    )
    .await;
    assert_eq!(nothing_left, Ok(Some(false)));
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

fn transfer_test_entries() -> BTreeMap<OplogIndex, OplogEntry> {
    [
        (OplogIndex::INITIAL, OplogEntry::no_op(None).rounded()),
        (OplogIndex::from_u64(2), OplogEntry::suspend().rounded()),
    ]
    .into_iter()
    .collect()
}

#[test]
async fn archive_transfer_verifies_destination_before_deleting_source(_tracing: &Tracing) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let expected = transfer_test_entries();
    let source = Arc::new(TransferTestArchive::new(
        "source",
        expected.clone(),
        events.clone(),
    ));
    let target = Arc::new(TransferTestArchive::new(
        "target",
        BTreeMap::new(),
        events.clone(),
    ));

    transfer_between_lower_layers(
        0,
        OplogIndex::from_u64(2),
        nev![
            source.clone() as Arc<dyn OplogArchive + Send + Sync>,
            target.clone() as Arc<dyn OplogArchive + Send + Sync>
        ],
    )
    .await;

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "source.read".to_string(),
            "target.append".to_string(),
            "target.verify".to_string(),
            "source.drop".to_string(),
        ]
    );
    assert_eq!(*target.entries.lock().unwrap(), expected);
    assert!(source.entries.lock().unwrap().is_empty());
}

#[test]
async fn archive_transfer_verification_failure_preserves_source(_tracing: &Tracing) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let expected = transfer_test_entries();
    let source = Arc::new(TransferTestArchive::new(
        "source",
        expected.clone(),
        events.clone(),
    ));
    let mut target = TransferTestArchive::new("target", BTreeMap::new(), events.clone());
    target.fail_verification = true;
    let target = Arc::new(target);

    assert_panics(transfer_between_lower_layers(
        0,
        OplogIndex::from_u64(2),
        nev![
            source.clone() as Arc<dyn OplogArchive + Send + Sync>,
            target as Arc<dyn OplogArchive + Send + Sync>
        ],
    ))
    .await;
    assert_eq!(*source.entries.lock().unwrap(), expected);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "source.read".to_string(),
            "target.append".to_string(),
            "target.verify".to_string(),
        ]
    );
}

#[test]
async fn archive_transfer_append_failure_preserves_source(_tracing: &Tracing) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let expected = transfer_test_entries();
    let source = Arc::new(TransferTestArchive::new(
        "source",
        expected.clone(),
        events.clone(),
    ));
    let mut target = TransferTestArchive::new("target", BTreeMap::new(), events.clone());
    target.fail_append = true;
    let target = Arc::new(target);

    assert_panics(transfer_between_lower_layers(
        0,
        OplogIndex::from_u64(2),
        nev![
            source.clone() as Arc<dyn OplogArchive + Send + Sync>,
            target as Arc<dyn OplogArchive + Send + Sync>
        ],
    ))
    .await;
    assert_eq!(*source.entries.lock().unwrap(), expected);
    assert_eq!(
        *events.lock().unwrap(),
        vec!["source.read".to_string(), "target.append".to_string()]
    );
}

#[test]
async fn compressed_transfer_verification_bypasses_append_cache(_tracing: &Tracing) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let expected = transfer_test_entries();
    let source = Arc::new(TransferTestArchive::new("source", expected.clone(), events));
    let indexed_storage = Arc::new(ReadCountingIndexedStorage::discarding_compressed_appends());
    let service =
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default());
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "uncached-transfer-verification".to_string(),
        },
    );
    let target = service
        .open_fresh(&owned_agent_id, AgentMode::Durable)
        .await;

    assert_panics(transfer_between_lower_layers(
        0,
        OplogIndex::from_u64(2),
        nev![
            source.clone() as Arc<dyn OplogArchive + Send + Sync>,
            target.clone()
        ],
    ))
    .await;
    assert_eq!(*source.entries.lock().unwrap(), expected);
    assert_eq!(
        target.read_source(OplogIndex::INITIAL, 2).await,
        expected,
        "the append-populated cache would have hidden the missing persisted chunk"
    );
    assert!(indexed_storage.reads() > 0);
}

#[test]
async fn blob_transfer_verifies_the_persisted_entry_representation(_tracing: &Tracing) {
    let entry = OplogEntry::NoOp {
        timestamp: "2026-08-27T13:09:36.123456Z".parse().unwrap(),
        entity_parent_start_index: None,
    };
    assert_ne!(entry, entry.clone().rounded());

    let expected = BTreeMap::from([(OplogIndex::INITIAL, entry.clone())]);
    let source = Arc::new(TransferTestArchive::new(
        "source",
        expected.clone(),
        Arc::new(std::sync::Mutex::new(Vec::new())),
    ));
    let target_service = BlobOplogArchiveService::new(Arc::new(InMemoryBlobStorage::new()), 2);
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "persisted-transfer-representation".to_string(),
        },
    );
    let target = target_service
        .open_fresh(&owned_agent_id, AgentMode::Ephemeral)
        .await;

    transfer_between_lower_layers(
        0,
        OplogIndex::INITIAL,
        nev![
            source.clone() as Arc<dyn OplogArchive + Send + Sync>,
            target.clone()
        ],
    )
    .await;

    assert!(source.entries.lock().unwrap().is_empty());
    assert_eq!(
        target.read_source(OplogIndex::INITIAL, 1).await,
        BTreeMap::from([(OplogIndex::INITIAL, entry.rounded())])
    );
}

#[test]
async fn deleting_worker_fences_in_flight_archive_transfers(_tracing: &Tracing) {
    deleting_worker_fences_in_flight_archive_transfers_impl(AgentMode::Durable).await;
}

#[test]
async fn deleting_ephemeral_worker_fences_in_flight_archive_transfers(_tracing: &Tracing) {
    deleting_worker_fences_in_flight_archive_transfers_impl(AgentMode::Ephemeral).await;
}

#[test]
async fn open_multilayer_oplog_retains_stale_index_after_service_deletion(_tracing: &Tracing) {
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
    let archive = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage,
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary.clone(), nev![archive], 100, 1);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "delete-with-open-oplog".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = service
        .create(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::no_op(None),
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    oplog.add_and_commit(OplogEntry::no_op(None)).await.unwrap();
    let current = oplog.current_oplog_index().await;

    service
        .delete(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
        )
        .await
        .unwrap();

    assert_eq!(oplog.current_oplog_index().await, current);
    assert!(!service.exists(&owned_agent_id, AgentMode::Durable).await);
    let panic = AssertUnwindSafe(oplog.read_exact(OplogIndex::INITIAL, current.as_u64()))
        .catch_unwind()
        .await
        .expect_err("exact read from the deleted storage must panic");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied());
    assert_eq!(
        message,
        Some("Oplog read failed: missing oplog entries in range [1..=2]")
    );

    oplog.stop_and_wait().await.unwrap();
    // Deletion must admit a fresh oplog generation, and dropping the
    // old generation afterwards must not evict either replacement cache entry.
    let metadata = make_agent_metadata(owned_agent_id.agent_id(), account_id, environment_id);
    let initial_entry = OplogEntry::NoOp {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
    };
    let replacement = service
        .create(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            initial_entry.clone(),
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert!(!Arc::ptr_eq(&oplog, &replacement));
    assert_eq!(replacement.current_oplog_index().await, OplogIndex::INITIAL);
    assert_eq!(
        replacement.read_exact(OplogIndex::INITIAL, 1).await,
        BTreeMap::from([(OplogIndex::INITIAL, initial_entry.rounded())])
    );

    let replacement_primary = primary
        .open(
            &mut primary.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::INITIAL),
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    drop(oplog);
    let reopened = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::INITIAL),
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert!(Arc::ptr_eq(&replacement, &reopened));
    let reopened_primary = primary
        .open(
            &mut primary.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogIndex::INITIAL),
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert!(Arc::ptr_eq(&replacement_primary, &reopened_primary));
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
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            agent_mode,
            None,
            AgentMetadata {
                agent_mode,
                ..make_agent_metadata(agent_id, account_id, environment_id)
            },
            default_last_known_status(),
            default_execution_status(agent_mode),
            None,
        )
        .await;

    oplog.add(OplogEntry::no_op(None)).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    if agent_mode == AgentMode::Ephemeral {
        EphemeralOplog::try_archive(&oplog)
            .await
            .expect("ephemeral oplog must be archivable");
    }
    tokio::time::timeout(Duration::from_secs(1), append_started_rx)
        .await
        .expect("archive transfer did not start")
        .expect("archive transfer start signal dropped");

    service
        .delete(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            agent_mode,
            None,
        )
        .await
        .unwrap();

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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    info!("FIRST OPEN DONE");

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
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
        oplog.add(entry.clone()).await.unwrap();
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
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
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await
    } else {
        oplog
    };

    let entries: Vec<OplogEntry> = (100..1000)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
                error: AgentError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect();

    for (n, entry) in entries.iter().enumerate() {
        oplog.add(entry.clone()).await.unwrap();
        if n % 100 == 0 {
            oplog.commit(CommitLevel::Always).await.unwrap();
        }
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
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
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await
    } else {
        oplog
    };

    oplog
        .add(
            OplogEntry::Error {
                timestamp,
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
                error: AgentError::Unknown("last".to_string()),
                retry_from: OplogIndex::NONE,
                inside_atomic_region: false,
                retry_policy_state: None,
            }
            .rounded(),
        )
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    drop(oplog);

    let entry1 = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await;
    let entry2 = oplog_service
        .read_exact(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::from_u64(100),
            1,
        )
        .await;
    let entry3 = oplog_service
        .read_exact(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::from_u64(1000),
            1,
        )
        .await;
    let entry4 = oplog_service
        .read_exact(
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
            entity_parent_start_index: None,
            kind: OplogErrorKind::Invocation,
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
            entity_parent_start_index: None,
            kind: OplogErrorKind::Invocation,
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
            entity_parent_start_index: None,
            kind: OplogErrorKind::Invocation,
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
            entity_parent_start_index: None,
            kind: OplogErrorKind::Invocation,
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
                    entity_parent_start_index: None,
                    kind: OplogErrorKind::Invocation,
                    error: AgentError::Unknown(i.to_string()),
                    retry_from: OplogIndex::NONE,
                    inside_atomic_region: false,
                    retry_policy_state: None,
                }
                .rounded()
            })
            .collect();

        for entry in &entries {
            oplog.add(entry.clone()).await.unwrap();
        }
        oplog.commit(CommitLevel::Always).await.unwrap();
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
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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

    // The primary key fences new creation even after all entries have been archived.
    assert!(primary_exists);
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
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
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
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await;
        for entry in &entries {
            oplog.add(entry.clone()).await.unwrap();
        }
        oplog.commit(CommitLevel::Always).await.unwrap();

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
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await;
        let result = MultiLayerOplog::try_archive(&oplog).await;
        drop(oplog);
        result
    };

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &mut primary_oplog_service
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
        let create_entry = create_test_entry(
            agent_id.clone(),
            AgentMode::Durable,
            ComponentRevision::new(1).unwrap(),
            environment_id,
            account_id,
            Uuid::new_v4(),
        );

        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let oplog = oplog_service
            .create(
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                AgentMode::Durable,
                create_entry,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
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
                            None,
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await
                        .unwrap();
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
                            None,
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await
                        .unwrap();
                }

                debug!("[{r:?}] => archiving {agent_id} to tertiary layer");
                MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!("Adding more oplog entries to primary");
                    oplog
                        .add_and_commit(OplogEntry::log(
                            None,
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await
                        .unwrap();
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
        let create_entry = create_test_entry(
            agent_id.clone(),
            mode,
            ComponentRevision::new(1).unwrap(),
            environment_id,
            account_id,
            Uuid::now_v7(),
        );
        let oplog = oplog_service
            .create(
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                mode,
                create_entry,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(mode),
                None,
            )
            .await;
        oplog.commit(CommitLevel::Always).await.unwrap();
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::from_u64(0),
                    end: OplogIndex::from_u64(0),
                },
            ),
            make_agent_metadata(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    initial_oplog.commit(CommitLevel::Always).await.unwrap();
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
                        &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                        &owned_agent_id,
                        AgentMode::Durable,
                        None,
                        make_agent_metadata(worker_id.clone(), account_id, environment_id),
                        default_last_known_status(),
                        default_execution_status(AgentMode::Durable),
                        None,
                    )
                    .await;

                // Each task adds an entry and commits. If two tasks ended up with
                // different oplog instances (due to the get_or_open race), they'll
                // have independent last_committed_idx and produce duplicate ids,
                // causing a unique key violation on commit.
                oplog.add(OplogEntry::suspend()).await.unwrap();
                // `add` is fallible now: commit can panic on unique key violation;
                // we use the Oplog trait method directly and let it propagate.
                oplog.commit(CommitLevel::Always).await.unwrap();

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
// Scan cursor encoding and phase transitions
// ---------------------------------------------------------------------------

#[test]
fn scan_cursor_initial_state_starts_in_durable_mode() {
    let state = decode_scan_cursor(&ScanCursor::default(), None).unwrap();
    assert_eq!(state.layer, 0);
    assert_eq!(state.mode, AgentMode::Durable);
    assert_eq!(state.resume, None);
}

#[test]
fn scan_cursor_round_trips_marker_resume() {
    let cursor = next_scan_cursor(
        OplogScanState {
            layer: 2,
            mode: AgentMode::Durable,
            resume: None,
        },
        None,
        Some(ScanResume::Marker("agent-key".to_string())),
    )
    .unwrap();

    assert!(cursor.as_str().starts_with(SCAN_CURSOR_PREFIX));
    assert_eq!(
        decode_scan_cursor(&cursor, None).unwrap(),
        OplogScanState {
            layer: 2,
            mode: AgentMode::Durable,
            resume: Some(ScanResume::Marker("agent-key".to_string())),
        }
    );
}

#[test]
fn scan_cursor_decodes_fixed_wire_format_fixtures() {
    let fixtures = [
        (
            "gsc1_eyJsYXllciI6MCwibW9kZSI6IkR1cmFibGUiLCJyZXN1bWUiOnsidHlwZSI6Im1hcmtlciIsInZhbHVlIjoiYWdlbnQta2V5In19",
            OplogScanState {
                layer: 0,
                mode: AgentMode::Durable,
                resume: Some(ScanResume::Marker("agent-key".to_string())),
            },
        ),
        (
            "gsc1_eyJsYXllciI6MiwibW9kZSI6IkVwaGVtZXJhbCIsInJlc3VtZSI6eyJ0eXBlIjoiY3Vyc29yIiwidmFsdWUiOjE3fX0",
            OplogScanState {
                layer: 2,
                mode: AgentMode::Ephemeral,
                resume: Some(ScanResume::Cursor(17)),
            },
        ),
        (
            "gsc1_eyJsYXllciI6MSwibW9kZSI6IkR1cmFibGUiLCJyZXN1bWUiOm51bGx9",
            OplogScanState {
                layer: 1,
                mode: AgentMode::Durable,
                resume: None,
            },
        ),
    ];

    for (token, expected) in fixtures {
        assert_eq!(
            encode_scan_cursor(expected.clone()).unwrap().as_str(),
            token
        );
        assert_eq!(
            decode_scan_cursor(&ScanCursor::new(token.to_string()), None).unwrap(),
            expected
        );
    }
}

#[test]
fn scan_cursor_durable_completion_advances_to_ephemeral() {
    let cursor = next_scan_cursor(
        OplogScanState {
            layer: 1,
            mode: AgentMode::Durable,
            resume: Some(ScanResume::Cursor(17)),
        },
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        decode_scan_cursor(&cursor, None).unwrap(),
        OplogScanState {
            layer: 1,
            mode: AgentMode::Ephemeral,
            resume: None,
        }
    );
}

#[test]
fn scan_cursor_single_or_ephemeral_mode_completion_is_terminal() {
    for (mode, modes) in [
        (AgentMode::Durable, Some(AgentMode::Durable)),
        (AgentMode::Ephemeral, Some(AgentMode::Ephemeral)),
        (AgentMode::Ephemeral, None),
    ] {
        let cursor = next_scan_cursor(
            OplogScanState {
                layer: 0,
                mode,
                resume: None,
            },
            modes,
            None,
        )
        .unwrap();
        assert!(cursor.is_finished());
    }
}

#[test]
fn scan_cursor_rejects_malformed_and_mode_mismatched_tokens() {
    for cursor in [
        ScanCursor::new("0/123".to_string()),
        ScanCursor::new("gsc1_%%%".to_string()),
        ScanCursor::new("gsc1_e30".to_string()),
    ] {
        assert!(decode_scan_cursor(&cursor, None).is_err());
    }

    let cursor = first_scan_cursor(0, Some(AgentMode::Ephemeral)).unwrap();
    assert!(decode_scan_cursor(&cursor, Some(AgentMode::Durable)).is_err());
}

#[test]
async fn oplog_services_reject_invalid_cursor_layers_and_resumes(_tracing: &Tracing) {
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
    let compressed: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage,
        1,
        RetryConfig::default(),
    ));
    let blob = Arc::new(BlobOplogArchiveService::new(blob_storage, 2));
    let multilayer = MultiLayerOplogService::new(
        primary.clone(),
        nev![compressed, blob.clone() as Arc<dyn OplogArchiveService>],
        1000,
        10,
    );
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    let layer_one = first_scan_cursor(1, Some(AgentMode::Durable)).unwrap();
    assert!(matches!(
        primary
            .scan_for_component(
                &environment_id,
                &component_id,
                Some(AgentMode::Durable),
                layer_one,
                1,
            )
            .await,
        Err(WorkerExecutorError::InvalidRequest { .. })
    ));

    let last_valid_layer = first_scan_cursor(2, Some(AgentMode::Durable)).unwrap();
    multilayer
        .scan_for_component(
            &environment_id,
            &component_id,
            Some(AgentMode::Durable),
            last_valid_layer,
            1,
        )
        .await
        .unwrap();
    let invalid_layer = first_scan_cursor(3, Some(AgentMode::Durable)).unwrap();
    assert!(matches!(
        multilayer
            .scan_for_component(
                &environment_id,
                &component_id,
                Some(AgentMode::Durable),
                invalid_layer,
                1,
            )
            .await,
        Err(WorkerExecutorError::InvalidRequest { .. })
    ));

    for resume in [
        ScanResume::Marker("marker".to_string()),
        ScanResume::Cursor(1),
    ] {
        let cursor = encode_scan_cursor(OplogScanState {
            layer: 2,
            mode: AgentMode::Durable,
            resume: Some(resume),
        })
        .unwrap();
        assert!(matches!(
            blob.scan_for_component(
                &environment_id,
                &component_id,
                Some(AgentMode::Durable),
                cursor,
                1,
            )
            .await,
            Err(WorkerExecutorError::InvalidRequest { .. })
        ));
    }
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

    let durable_create = create_test_entry(
        agent_id.clone(),
        AgentMode::Durable,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::now_v7(),
    )
    .rounded();
    let ephemeral_create = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(2).unwrap(),
        environment_id,
        account_id,
        Uuid::now_v7(),
    )
    .rounded();

    let durable_oplog = oplog_service
        .create(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            durable_create.clone(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    let ephemeral_oplog = oplog_service
        .create(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            ephemeral_create.clone(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
        )
        .await;
    durable_oplog.commit(CommitLevel::Always).await.unwrap();
    ephemeral_oplog.commit(CommitLevel::Always).await.unwrap();

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
        .read_exact(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .into_values()
        .next()
        .expect("expected one durable entry");
    let ephemeral_first = oplog_service
        .read_exact(
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
        .delete(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
        )
        .await
        .unwrap();
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
        let create_entry = create_test_entry(
            agent_id.clone(),
            mode,
            ComponentRevision::new(1).unwrap(),
            environment_id,
            account_id,
            Uuid::now_v7(),
        );
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
        let oplog = oplog_service
            .create(
                &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
                &owned_agent_id,
                mode,
                create_entry,
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(mode),
                None,
            )
            .await;
        oplog.commit(CommitLevel::Always).await.unwrap();
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
        match decode_scan_cursor(&cursor, None).unwrap().mode {
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

#[test]
async fn owned_payload_upload_preserves_allocation_at_inline_threshold_and_roundtrips(
    _tracing: &Tracing,
) {
    let inline = vec![1_u8; 64];
    let max_payload_size = serialize(&inline).unwrap().len();
    let external = vec![2_u8; 65];
    assert!(serialize(&external).unwrap().len() > max_payload_size);

    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        max_payload_size,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "owned-payload".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let inline_ptr = inline.as_ptr();
    let inline_without_cache = match oplog.upload_payload_owned(inline).await.unwrap() {
        OplogPayload::SerializedInline {
            bytes,
            cached: Some(cached),
        } => {
            assert_eq!(cached.as_ptr(), inline_ptr);
            OplogPayload::SerializedInline {
                bytes,
                cached: None,
            }
        }
        other => panic!("expected an inline payload with a cache, got {other:?}"),
    };

    let external_ptr = external.as_ptr();
    let external_without_cache = match oplog.upload_payload_owned(external).await.unwrap() {
        OplogPayload::External {
            payload_id,
            md5_hash,
            cached: Some(cached),
        } => {
            assert_eq!(cached.as_ptr(), external_ptr);
            OplogPayload::External {
                payload_id,
                md5_hash,
                cached: None,
            }
        }
        other => panic!("expected an external payload with a cache, got {other:?}"),
    };

    let reopened = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    assert_eq!(
        reopened
            .download_payload::<Vec<u8>>(inline_without_cache)
            .await
            .unwrap(),
        vec![1_u8; 64]
    );
    assert_eq!(
        reopened
            .download_payload::<Vec<u8>>(external_without_cache)
            .await
            .unwrap(),
        vec![2_u8; 65]
    );
}

#[test]
async fn owned_snapshot_payloads_persist_and_replay_across_inline_threshold(_tracing: &Tracing) {
    let inline = vec![3_u8; 64];
    let inline_ptr = inline.as_ptr();
    let max_payload_size = serialize(&inline).unwrap().len();
    let external = vec![4_u8; 65];
    let external_ptr = external.as_ptr();
    assert!(serialize(&external).unwrap().len() > max_payload_size);

    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(
        indexed_storage,
        blob_storage,
        1,
        1,
        max_payload_size,
        RetryConfig::default(),
    )
    .await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "owned-snapshot-payload".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let inline_description = oplog
        .create_snapshot_based_update_description(
            ComponentRevision::new(2).unwrap(),
            inline,
            "application/inline".to_string(),
        )
        .await
        .unwrap();
    let UpdateDescription::SnapshotBased {
        payload:
            OplogPayload::SerializedInline {
                cached: Some(cached),
                ..
            },
        ..
    } = &inline_description
    else {
        panic!("payload at the size threshold must be stored inline")
    };
    assert_eq!(cached.as_ptr(), inline_ptr);

    let external_description = oplog
        .create_snapshot_based_update_description(
            ComponentRevision::new(3).unwrap(),
            external,
            "application/external".to_string(),
        )
        .await
        .unwrap();
    let UpdateDescription::SnapshotBased {
        payload: OplogPayload::External {
            cached: Some(cached),
            ..
        },
        ..
    } = &external_description
    else {
        panic!("payload above the size threshold must be stored externally")
    };
    assert_eq!(cached.as_ptr(), external_ptr);

    let inline_index = oplog
        .add(OplogEntry::PendingUpdate {
            timestamp: Timestamp::now_utc(),
            description: inline_description,
        })
        .await
        .unwrap();
    let external_index = oplog
        .add(OplogEntry::PendingUpdate {
            timestamp: Timestamp::now_utc(),
            description: external_description,
        })
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let persisted = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, inline_index, 2)
        .await;
    let inline_description = match persisted.get(&inline_index).unwrap() {
        OplogEntry::PendingUpdate {
            description:
                UpdateDescription::SnapshotBased {
                    payload: OplogPayload::SerializedInline { cached: None, .. },
                    ..
                },
            ..
        } => persisted.get(&inline_index).unwrap().clone(),
        other => panic!("expected an uncached inline snapshot after persistence, got {other:?}"),
    };
    let external_description = match persisted.get(&external_index).unwrap() {
        OplogEntry::PendingUpdate {
            description:
                UpdateDescription::SnapshotBased {
                    payload: OplogPayload::External { cached: None, .. },
                    ..
                },
            ..
        } => persisted.get(&external_index).unwrap().clone(),
        other => panic!("expected an uncached external snapshot after persistence, got {other:?}"),
    };

    for (entry, expected_payload, expected_mime) in [
        (inline_description, vec![3_u8; 64], "application/inline"),
        (external_description, vec![4_u8; 65], "application/external"),
    ] {
        let OplogEntry::PendingUpdate { description, .. } = entry else {
            unreachable!()
        };
        let (payload, mime_type) = oplog
            .get_upload_description_payload(description)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(payload, expected_payload);
        assert_eq!(mime_type, expected_mime);
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
            invocation_id: None,
            observational_owner: None,
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::ReadRemote,
        })
        .await
        .unwrap();
    assert_eq!(start_idx, last_oplog_idx.next());

    oplog.commit(CommitLevel::Always).await.unwrap();

    // Read back from the service (storage), so the payload reference carries no in-memory cache and
    // the download must hit blob storage.
    let entries = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, start_idx, 1)
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;

    let request = HostRequest::Custom("request".into_typed_schema_value().unwrap());

    let last_oplog_idx = oplog.current_oplog_index().await;
    let (start_idx, pending) = oplog
        .add_start_with_reserved_payload(request, |request_payload| OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("f".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::ReadRemote,
        })
        .await
        .unwrap();
    assert_eq!(start_idx, last_oplog_idx.next());
    // Inline payloads are already durable: waiting is a no-op.
    pending.wait().await.unwrap();

    oplog.commit(CommitLevel::Always).await.unwrap();

    let entries = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, start_idx, 1)
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
        invocation_id: None,
        observational_owner: None,
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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
    oplog.commit(CommitLevel::Always).await.unwrap();

    let entries = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, first_idx, 2)
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
    let create_entry = create_test_entry(
        agent_id.clone(),
        AgentMode::Ephemeral,
        ComponentRevision::new(1).unwrap(),
        environment_id,
        account_id,
        Uuid::new_v4(),
    )
    .rounded();
    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let oplog = oplog_service
        .create(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Ephemeral,
            create_entry,
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
            None,
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
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
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

    oplog.commit(CommitLevel::Always).await.unwrap();

    // Read back through the service stack (no in-memory cache).
    let entries = oplog_service
        .read_exact(&owned_agent_id, AgentMode::Durable, large_idx, 2)
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

/// The fence, end to end through the real oplog service, on SQLite.
async fn fencing_oplog_service(tempdir: &tempfile::TempDir, name: &str) -> PrimaryOplogService {
    let config = golem_common::config::DbSqliteConfig {
        database: tempdir
            .path()
            .join(format!("{name}.db"))
            .to_string_lossy()
            .into_owned(),
        max_connections: 4,
        foreign_keys: false,
    };
    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> =
        Arc::new(SqliteIndexedStorage::configured(&config).await.unwrap());
    PrimaryOplogService::new(
        indexed_storage,
        Arc::new(InMemoryBlobStorage::new()),
        100,
        1,
        128,
        RetryConfig::default(),
    )
    .await
}

#[test]
async fn an_oplog_opened_at_the_owning_epoch_can_be_written(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let service = fencing_oplog_service(&tempdir, "owning").await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "owned".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(7)),
        )
        .await;

    // Opening records the epoch, so the writes that follow are accepted.
    oplog.add(OplogEntry::suspend().rounded()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(oplog.length().await, 1);
}

#[test]
async fn an_oplog_opened_at_a_stale_epoch_refuses_every_write(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "moved".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    // Two services over one database, because that is what two executors are. A single service
    // would not do: `OpenOplogs` caches by agent id, so a second `open` on it hands back the
    // first oplog - epoch and all - instead of constructing a new one.
    let owning_executor = fencing_oplog_service(&tempdir, "shared").await;
    let losing_executor = fencing_oplog_service(&tempdir, "shared").await;

    // The shard's new owner takes it over at a higher epoch and writes.
    let owner = owning_executor
        .open(
            &mut owning_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(9)),
        )
        .await;
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();

    // The executor that lost the shard still believes it holds epoch 8. It is refused at its
    // very first write, and told which epoch owns the oplog now.
    let loser = losing_executor
        .open(
            &mut losing_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(8)),
        )
        .await;
    // `add` only buffers; the storage write happens at the commit, so the refusal is asserted
    // over the pair rather than over `add` alone.
    let write = async {
        loser.add(OplogEntry::exited().rounded()).await?;
        loser.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    match write {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.agent_id, agent_id);
            assert_eq!(fence.expected_epoch, golem_common::model::ShardEpoch(8));
            assert_eq!(
                fence.actual_epoch,
                Some(golem_common::model::ShardEpoch(9)),
                "the fence must name the epoch that owns the oplog now"
            );
        }
        other => panic!("expected the write to be fenced, got {other:?}"),
    }

    // ... and stays refused: the oplog is poisoned, so it does not even ask the storage again.
    assert!(matches!(
        loser.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    assert_eq!(
        owner.length().await,
        1,
        "the losing executor must not have appended to the owner's oplog"
    );
}

#[test]
async fn an_oplog_opened_without_an_epoch_asserts_nothing(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let service = fencing_oplog_service(&tempdir, "unfenced").await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "unfenced".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    // `None` is what the debugging service and a fork of a remote target pass: no ownership
    // claim, so the record is neither written nor checked.
    let oplog = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    oplog.add(OplogEntry::suspend().rounded()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(oplog.length().await, 1);
}

#[test]
async fn deleting_an_oplog_fences_a_writer_that_still_holds_it(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let service = fencing_oplog_service(&tempdir, "deleted").await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "deleted".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let oplog = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(7)),
        )
        .await;
    oplog.add(OplogEntry::suspend().rounded()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    service
        .delete(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
        )
        .await
        .unwrap();

    // The handle outlives the delete, as a zombie executor's would. Its epoch is still the one
    // the record held, so only the record's absence can refuse it - an entry landing here would
    // bring back an oplog that was deleted.
    let write = async {
        oplog.add(OplogEntry::exited().rounded()).await?;
        oplog.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    match write {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.agent_id, agent_id);
            assert_eq!(fence.expected_epoch, golem_common::model::ShardEpoch(7));
            assert_eq!(
                fence.actual_epoch, None,
                "the record must be gone, not moved to another epoch"
            );
        }
        other => panic!("expected the write to be fenced, got {other:?}"),
    }
    assert!(
        !service.exists(&owned_agent_id, AgentMode::Durable).await,
        "the refused write must not have brought the deleted oplog back"
    );
}

#[test]
async fn a_fenced_oplog_is_not_handed_out_again_while_it_is_still_held(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "regained".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let executor = fencing_oplog_service(&tempdir, "shared").await;
    let other_executor = fencing_oplog_service(&tempdir, "shared").await;
    let open = |service: &PrimaryOplogService, epoch: u64| {
        let service = service.clone();
        let agent_id = agent_id.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .open(
                    &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
                    &owned_agent_id,
                    AgentMode::Durable,
                    None,
                    make_agent_metadata(agent_id, account_id, environment_id),
                    default_last_known_status(),
                    default_execution_status(AgentMode::Durable),
                    Some(golem_common::model::ShardEpoch(epoch)),
                )
                .await
        }
    };

    // This executor holds the shard at epoch 8, loses it to the other executor at 9, and is
    // refused - the handle is fenced and stays open, as a worker still stopping keeps it.
    let fenced = open(&executor, 8).await;
    let owner = open(&other_executor, 9).await;
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();
    fenced.add(OplogEntry::exited().rounded()).await.unwrap();
    assert!(matches!(
        fenced.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    drop(owner);

    // Re-granted the shard at 10 while the fenced handle is still alive, the executor opens the
    // agent again - recovering it - and must not be handed the finished handle: that one refuses
    // every write, and its view of the oplog stops where its refused entries began.
    let regained = open(&executor, 10).await;
    assert!(
        !Arc::ptr_eq(&regained, &fenced),
        "the fenced handle was handed out again"
    );
    regained.add(OplogEntry::exited().rounded()).await.unwrap();
    regained.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(regained.length().await, 2);

    // Dropping the fenced handle runs its remover; it must not evict the fresh handle, or the
    // next open would construct a third, and two live writers would share one oplog.
    drop(fenced);
    let again = open(&executor, 10).await;
    assert!(
        Arc::ptr_eq(&again, &regained),
        "the fresh handle was evicted by the fenced handle's removal"
    );
}

/// A below-threshold add on a moved shard only buffers, so nothing refuses it until the commit;
/// the refused commit latches the fence, and from then on the add itself is refused rather than
/// buffered under an index that could never reach the storage.
#[test]
async fn a_below_threshold_add_on_a_moved_shard_is_refused_once_the_commit_latches_the_fence(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "moved".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let executor = fencing_oplog_service(&tempdir, "shared").await;
    let other_executor = fencing_oplog_service(&tempdir, "shared").await;
    let open = |service: &PrimaryOplogService, epoch: u64| {
        let service = service.clone();
        let agent_id = agent_id.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .open(
                    &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
                    &owned_agent_id,
                    AgentMode::Durable,
                    None,
                    make_agent_metadata(agent_id, account_id, environment_id),
                    default_last_known_status(),
                    default_execution_status(AgentMode::Durable),
                    Some(ShardEpoch(epoch)),
                )
                .await
        }
    };

    let stale = open(&executor, 8).await;
    let owner = open(&other_executor, 9).await;
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(stale.fence(), None, "nothing has been refused yet");

    stale
        .add(OplogEntry::exited().rounded())
        .await
        .expect("a below-threshold add only buffers, so the moved shard does not refuse it");
    assert!(matches!(
        stale.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    let fence = stale
        .fence()
        .expect("the refused commit must latch the fence before it returns");
    assert_eq!(fence.expected_epoch, ShardEpoch(8));
    assert_eq!(fence.actual_epoch, Some(ShardEpoch(9)));

    assert!(
        matches!(
            stale.add(OplogEntry::exited().rounded()).await,
            Err(OplogError::Fenced(_))
        ),
        "a latched fence refuses a below-threshold add"
    );
    assert!(matches!(
        stale.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    assert_eq!(
        stale.fence(),
        Some(fence),
        "the latch keeps the first refusal"
    );
}

#[test]
async fn an_executor_that_loses_the_shard_mid_flight_is_refused_at_its_next_write(
    _tracing: &Tracing,
) {
    // The realistic sequence, and the one only the per-write assertion can catch: this executor
    // opened the oplog while it still owned the shard, so its epoch record went in cleanly and
    // nothing was poisoned at open. The shard moves underneath it afterwards.
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "mid-flight".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

    let losing_executor = fencing_oplog_service(&tempdir, "mid-flight").await;
    let gaining_executor = fencing_oplog_service(&tempdir, "mid-flight").await;

    let loser = losing_executor
        .open(
            &mut losing_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(4)),
        )
        .await;
    loser.add(OplogEntry::suspend().rounded()).await.unwrap();
    loser.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(loser.length().await, 1, "it owned the shard at this point");

    // The shard is re-granted to another executor, which opens the oplog at the new epoch.
    let _gainer = gaining_executor
        .open(
            &mut gaining_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(5)),
        )
        .await;

    // The loser's already-open oplog is not poisoned - it had no reason to be - so this is the
    // per-write epoch assertion doing the work, and nothing of its is written.
    let write = async {
        loser.add(OplogEntry::exited().rounded()).await?;
        loser.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    match write {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.expected_epoch, golem_common::model::ShardEpoch(4));
            assert_eq!(fence.actual_epoch, Some(golem_common::model::ShardEpoch(5)));
        }
        other => panic!("expected the in-flight write to be fenced, got {other:?}"),
    }
    assert_eq!(
        loser.length().await,
        1,
        "the refused entry must not be there"
    );
}

#[test]
async fn wait_for_replicas_does_not_report_a_fenced_flush_as_durable(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "flushed".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let losing_executor = fencing_oplog_service(&tempdir, "flushed").await;
    let owning_executor = fencing_oplog_service(&tempdir, "flushed").await;

    let loser = losing_executor
        .open(
            &mut losing_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(8)),
        )
        .await;
    let owner = owning_executor
        .open(
            &mut owning_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(9)),
        )
        .await;
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();

    // The guest's `oplog-commit` path: the entry is only buffered, and the flush inside
    // `wait_for_replicas` is what the storage refuses. The fencing backends have no replicas to
    // wait for, so a count taken after the refusal would read as a successful commit.
    loser.add(OplogEntry::exited().rounded()).await.unwrap();
    assert!(
        !loser.wait_for_replicas(1, Duration::from_secs(1)).await,
        "a flush the storage refused must not be reported as durable"
    );
    match loser.fence() {
        Some(fence) => assert_eq!(
            fence.actual_epoch,
            Some(golem_common::model::ShardEpoch(9)),
            "the fence must name the epoch that owns the oplog now"
        ),
        None => panic!("the refused flush must latch the fence"),
    }
    assert_eq!(
        owner.length().await,
        1,
        "the losing executor must not have appended to the owner's oplog"
    );
}

#[test]
async fn a_fenced_oplog_refuses_new_adds_and_keeps_the_indices_it_handed_out_readable(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "half-alive".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let losing_executor = fencing_oplog_service(&tempdir, "half-alive").await;
    let owning_executor = fencing_oplog_service(&tempdir, "half-alive").await;

    let loser = losing_executor
        .open(
            &mut losing_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(8)),
        )
        .await;
    let owner = owning_executor
        .open(
            &mut owning_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(golem_common::model::ShardEpoch(9)),
        )
        .await;
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();

    // Below the commit threshold both adds only buffer, and each is answered with an index.
    let first = loser.add(OplogEntry::suspend().rounded()).await.unwrap();
    loser.add(OplogEntry::exited().rounded()).await.unwrap();
    // A reader takes the horizon before the storage refuses the batch and reads after it, as a
    // durable session or a fork running beside the invocation loop does.
    let horizon = loser.current_oplog_index().await;
    assert!(matches!(
        loser.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    let entries = loser
        .read_exact(first, horizon.as_u64() - first.as_u64() + 1)
        .await;
    assert_eq!(
        entries.len(),
        2,
        "an index handed out before the refusal must still be readable after it"
    );

    // Once latched, an add below the threshold is refused instead of buffered under an index
    // that could never reach the storage.
    assert!(matches!(
        loser.add(OplogEntry::suspend().rounded()).await,
        Err(OplogError::Fenced(_))
    ));
    assert!(matches!(
        loser
            .add_pair(
                OplogEntry::suspend().rounded(),
                Box::new(|_| OplogEntry::exited().rounded())
            )
            .await,
        Err(OplogError::Fenced(_))
    ));
    assert_eq!(
        loser.current_oplog_index().await,
        horizon,
        "a refused add must not take an index"
    );
    assert!(matches!(
        loser.commit(CommitLevel::Always).await,
        Err(OplogError::Fenced(_))
    ));
    assert_eq!(
        owner.length().await,
        1,
        "the losing executor must not have appended to the owner's oplog"
    );
}

#[test]
async fn an_opener_at_a_newer_epoch_is_not_handed_the_older_generations_handle(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "came-back".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let executor = fencing_oplog_service(&tempdir, "came-back").await;
    let open = |epoch: u64| {
        let service = executor.clone();
        let agent_id = agent_id.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .open(
                    &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
                    &owned_agent_id,
                    AgentMode::Durable,
                    None,
                    make_agent_metadata(agent_id, account_id, environment_id),
                    default_last_known_status(),
                    default_execution_status(AgentMode::Durable),
                    Some(ShardEpoch(epoch)),
                )
                .await
        }
    };

    // The shard left this executor at epoch 5 and came back at 7 while the epoch-5 handle is still
    // held. Nothing has fenced that handle - nobody has written since - so only the epoch it was
    // opened with tells the cache it belongs to the older generation.
    let old = open(5).await;
    let new = open(7).await;
    assert!(
        !Arc::ptr_eq(&new, &old),
        "the opener at epoch 7 was handed the handle opened at 5"
    );
    assert_eq!(new.shard_epoch(), Some(ShardEpoch(7)));
    new.add(OplogEntry::suspend().rounded()).await.unwrap();
    new.commit(CommitLevel::Always).await.unwrap();

    let write = async {
        old.add(OplogEntry::exited().rounded()).await?;
        old.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    match write {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.expected_epoch, ShardEpoch(5));
            assert_eq!(fence.actual_epoch, Some(ShardEpoch(7)));
        }
        other => panic!("expected the older generation's write to be fenced, got {other:?}"),
    }

    // An equal or older request is handed the current handle. Building another would put two
    // live writers on epoch 7, and they would collide on the oplog's keys.
    let again = open(7).await;
    assert!(
        Arc::ptr_eq(&again, &new),
        "an opener at the same epoch must share the handle"
    );
    let stale = open(5).await;
    assert!(
        Arc::ptr_eq(&stale, &new),
        "an opener at an older epoch must not evict the newer handle"
    );
}

#[test]
async fn an_ephemeral_handle_is_reused_whatever_epoch_is_requested(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let primary = Arc::new(fencing_oplog_service(&tempdir, "ephemeral").await);
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        Arc::new(InMemoryIndexedStorage::new()),
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary, nev![archive], 10, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "ephemeral".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let mut metadata = make_agent_metadata(agent_id, account_id, environment_id);
    metadata.agent_mode = AgentMode::Ephemeral;
    let open = |epoch: u64| {
        let service = service.clone();
        let owned_agent_id = owned_agent_id.clone();
        let metadata = metadata.clone();
        async move {
            service
                .open(
                    &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
                    &owned_agent_id,
                    AgentMode::Ephemeral,
                    None,
                    metadata,
                    default_last_known_status(),
                    default_execution_status(AgentMode::Ephemeral),
                    Some(ShardEpoch(epoch)),
                )
                .await
        }
    };

    // An ephemeral handle asserts no epoch whatever it was opened with, so it belongs to no
    // ownership generation and a newer request is no reason to rebuild it.
    let first = open(5).await;
    assert_eq!(first.shard_epoch(), None);
    let second = open(7).await;
    assert!(
        Arc::ptr_eq(&second, &first),
        "an ephemeral handle was rebuilt for a newer epoch"
    );
}

#[test]
async fn a_fork_target_handle_is_not_reused_by_the_owners_first_open(_tracing: &Tracing) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let primary = Arc::new(fencing_oplog_service(&tempdir, "fork-target").await);
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        Arc::new(InMemoryIndexedStorage::new()),
        1,
        RetryConfig::default(),
    ));
    // Through the layered service, as production opens it: each layer caches its own handle, and
    // every one of them has to decline the fork's.
    let service = MultiLayerOplogService::new(primary, nev![archive], 10, 10);
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "fork-target".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create_entry = OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
        agent_id: agent_id.clone(),
        owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
        agent_mode: AgentMode::Durable,
        component_revision: ComponentRevision::new(1).unwrap(),
        env: Vec::new(),
        environment_id,
        created_by: account_id,
        parent: None,
        component_size: 100,
        initial_total_linear_memory_size: 100,
        initial_active_plugins: HashSet::new(),
        local_agent_config: Vec::new(),
        original_phantom_id: None,
        instance_id: Uuid::new_v4(),
    }))
    .rounded();

    // The fork copies into the target through a handle that asserts no epoch, since the target's
    // shard may belong to another executor. Here that handle is still held when the owner opens
    // the target.
    let forked = service
        .create(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            create_entry,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await;
    forked.add(OplogEntry::suspend().rounded()).await.unwrap();
    forked.commit(CommitLevel::Always).await.unwrap();

    let owner = service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(4)),
        )
        .await;
    assert!(
        !Arc::ptr_eq(&owner, &forked),
        "the owner's first open was handed the fork's unfenced handle"
    );
    assert_eq!(owner.shard_epoch(), Some(ShardEpoch(4)));
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();

    // Another executor takes the shard at epoch 5. The owner's next write is refused only if its
    // open recorded epoch 4; through the fork's handle it would have recorded nothing and been
    // accepted.
    let other_executor = fencing_oplog_service(&tempdir, "fork-target").await;
    let other = other_executor
        .open(
            &mut other_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(5)),
        )
        .await;
    other.add(OplogEntry::suspend().rounded()).await.unwrap();
    other.commit(CommitLevel::Always).await.unwrap();

    let write = async {
        owner.add(OplogEntry::exited().rounded()).await?;
        owner.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    match write {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.expected_epoch, ShardEpoch(4));
            assert_eq!(fence.actual_epoch, Some(ShardEpoch(5)));
        }
        other => panic!("expected the owner's write to be fenced, got {other:?}"),
    }
}

#[test]
async fn an_owner_opening_on_a_stale_last_index_starts_after_the_losers_last_write(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "stale-last-index".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let owning_executor = fencing_oplog_service(&tempdir, "shared").await;
    let losing_executor = fencing_oplog_service(&tempdir, "shared").await;

    let loser = losing_executor
        .open(
            &mut losing_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(5)),
        )
        .await;
    loser.add(OplogEntry::suspend().rounded()).await.unwrap();
    loser.commit(CommitLevel::Always).await.unwrap();

    // A layer above the primary reads the last index before the primary claims the epoch, as the
    // layered service does, and the losing executor commits again in that window: the record
    // still says 5, so the write is accepted.
    let stale_last_index = owning_executor
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;
    assert_eq!(stale_last_index, OplogIndex::from_u64(1));
    loser.add(OplogEntry::suspend().rounded()).await.unwrap();
    loser.commit(CommitLevel::Always).await.unwrap();

    let owner = owning_executor
        .open(
            &mut owning_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            Some(stale_last_index),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(6)),
        )
        .await;
    // Starting from the stale index, this append would reuse the loser's id and fail-stop the
    // executor that owns the shard.
    owner.add(OplogEntry::exited().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(owner.length().await, 3);
    assert_eq!(owner.current_oplog_index().await, OplogIndex::from_u64(3));

    let write = async {
        loser.add(OplogEntry::exited().rounded()).await?;
        loser.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    match write {
        Err(OplogError::Fenced(fence)) => {
            assert_eq!(fence.expected_epoch, ShardEpoch(5));
            assert_eq!(fence.actual_epoch, Some(ShardEpoch(6)));
        }
        other => panic!("expected the losing executor's write to be fenced, got {other:?}"),
    }
}

#[test]
async fn a_stale_create_of_an_oplog_the_owner_already_created_is_fenced_not_fatal(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "created-twice".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let owning_executor = fencing_oplog_service(&tempdir, "shared").await;
    let losing_executor = fencing_oplog_service(&tempdir, "shared").await;
    let create_entry = || {
        OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: agent_id.clone(),
            owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Durable,
            component_revision: ComponentRevision::new(1).unwrap(),
            env: Vec::new(),
            environment_id,
            created_by: account_id,
            parent: None,
            component_size: 100,
            initial_total_linear_memory_size: 100,
            initial_active_plugins: HashSet::new(),
            local_agent_config: Vec::new(),
            original_phantom_id: None,
            instance_id: Uuid::new_v4(),
        }))
        .rounded()
    };

    let owner = owning_executor
        .create(
            &mut owning_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            create_entry(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(6)),
        )
        .await;
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();

    // An executor that lost the shard creates the same agent. Its claim is refused, so it writes
    // nothing and must not fail-stop over an oplog that belongs to the owner.
    let stale = losing_executor
        .create(
            &mut losing_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            create_entry(),
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(5)),
        )
        .await;
    match stale.fence() {
        Some(fence) => {
            assert_eq!(fence.expected_epoch, ShardEpoch(5));
            assert_eq!(fence.actual_epoch, Some(ShardEpoch(6)));
        }
        None => panic!("the refused create must hand back a fenced oplog"),
    }
    let write = async {
        stale.add(OplogEntry::exited().rounded()).await?;
        stale.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    assert!(
        matches!(write, Err(OplogError::Fenced(_))),
        "expected the stale create's write to be fenced, got {write:?}"
    );
    assert_eq!(
        owner.length().await,
        2,
        "the stale create must not have appended to the owner's oplog"
    );
}

/// Keeps every fence it is told of, in order.
#[derive(Default)]
struct RecordingFenceObserver {
    fences: StdMutex<Vec<OplogFence>>,
}

impl RecordingFenceObserver {
    fn fences(&self) -> Vec<OplogFence> {
        self.fences.lock().unwrap().clone()
    }
}

impl OplogFenceObserver for RecordingFenceObserver {
    fn fenced(&self, fence: &OplogFence) {
        self.fences.lock().unwrap().push(fence.clone());
    }
}

fn initial_create_entry(
    agent_id: &AgentId,
    environment_id: EnvironmentId,
    account_id: AccountId,
) -> OplogEntry {
    OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
        agent_id: agent_id.clone(),
        owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
        agent_mode: AgentMode::Durable,
        component_revision: ComponentRevision::new(1).unwrap(),
        env: Vec::new(),
        environment_id,
        created_by: account_id,
        parent: None,
        component_size: 100,
        initial_total_linear_memory_size: 100,
        initial_active_plugins: HashSet::new(),
        local_agent_config: Vec::new(),
        original_phantom_id: None,
        instance_id: Uuid::new_v4(),
    }))
    .rounded()
}

#[test]
async fn a_refused_open_or_create_reports_the_stored_epoch_to_the_fence_observer(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let recorder = Arc::new(RecordingFenceObserver::default());
    let owning_executor = fencing_oplog_service(&tempdir, "shared").await;
    let losing_executor = fencing_oplog_service(&tempdir, "shared")
        .await
        .with_fence_observer(recorder.clone());
    let opened = AgentId {
        component_id: ComponentId::new(),
        agent_id: "opened-by-the-loser".into(),
    };
    let created = AgentId {
        component_id: ComponentId::new(),
        agent_id: "created-by-the-loser".into(),
    };
    let expected_fence = |agent_id: &AgentId| OplogFence {
        agent_id: agent_id.clone(),
        expected_epoch: ShardEpoch(5),
        actual_epoch: Some(ShardEpoch(6)),
        writer_conflict: false,
    };

    for agent_id in [&opened, &created] {
        let owner = owning_executor
            .create(
                &mut owning_executor
                    .lock_lifecycle(&OwnedAgentId::new(environment_id, agent_id).agent_id)
                    .await,
                &OwnedAgentId::new(environment_id, agent_id),
                AgentMode::Durable,
                initial_create_entry(agent_id, environment_id, account_id),
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                Some(ShardEpoch(6)),
            )
            .await;
        owner.add(OplogEntry::suspend().rounded()).await.unwrap();
        owner.commit(CommitLevel::Always).await.unwrap();
    }

    let stale = losing_executor
        .open(
            &mut losing_executor
                .lock_lifecycle(&OwnedAgentId::new(environment_id, &opened).agent_id)
                .await,
            &OwnedAgentId::new(environment_id, &opened),
            AgentMode::Durable,
            None,
            make_agent_metadata(opened.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(5)),
        )
        .await;
    assert!(stale.fence().is_some(), "the stale open was not refused");
    let reported = recorder.fences();
    assert!(
        !reported.is_empty(),
        "the refused open reported nothing to the observer"
    );
    for fence in &reported {
        assert_eq!(fence, &expected_fence(&opened));
    }

    // Born fenced, so its writes fail on the latch without asking the storage again.
    let write = async {
        stale.add(OplogEntry::exited().rounded()).await?;
        stale.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    assert!(
        matches!(write, Err(OplogError::Fenced(_))),
        "expected the stale open's write to be fenced, got {write:?}"
    );
    assert_eq!(
        recorder.fences().len(),
        reported.len(),
        "a write refused by the latch reported a refusal the storage never made"
    );

    // On a cache miss a refused create is reported twice, by `create` and by the open behind it,
    // and the observer merges. So what is asserted is what was learned, not how often.
    let reported_before_create = recorder.fences().len();
    let stale_create = losing_executor
        .create(
            &mut losing_executor
                .lock_lifecycle(&OwnedAgentId::new(environment_id, &created).agent_id)
                .await,
            &OwnedAgentId::new(environment_id, &created),
            AgentMode::Durable,
            initial_create_entry(&created, environment_id, account_id),
            make_agent_metadata(created.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(5)),
        )
        .await;
    assert!(
        stale_create.fence().is_some(),
        "the stale create was not refused"
    );
    let reported = recorder.fences().split_off(reported_before_create);
    assert!(
        !reported.is_empty(),
        "the refused create reported nothing to the observer"
    );
    for fence in &reported {
        assert_eq!(fence, &expected_fence(&created));
    }
}

#[test]
async fn a_create_refused_behind_a_cached_handle_still_reports_the_stored_epoch(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "created-again-while-held".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let recorder = Arc::new(RecordingFenceObserver::default());
    let owning_executor = fencing_oplog_service(&tempdir, "shared").await;
    let losing_executor = fencing_oplog_service(&tempdir, "shared")
        .await
        .with_fence_observer(recorder.clone());
    let create_at_5 = || async {
        losing_executor
            .create(
                &mut losing_executor
                    .lock_lifecycle(&owned_agent_id.agent_id)
                    .await,
                &owned_agent_id,
                AgentMode::Durable,
                initial_create_entry(&agent_id, environment_id, account_id),
                make_agent_metadata(agent_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                Some(ShardEpoch(5)),
            )
            .await
    };

    // Created while this executor owned the shard, and held without a write.
    let held = create_at_5().await;
    assert!(held.fence().is_none());
    assert!(recorder.fences().is_empty());

    // The shard moves, and its new owner claims the oplog.
    let owner = owning_executor
        .open(
            &mut owning_executor
                .lock_lifecycle(&owned_agent_id.agent_id)
                .await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(6)),
        )
        .await;
    assert!(owner.fence().is_none());

    // The claim is refused, and the open behind it hands back the held handle without asking the
    // storage, so the refusal `create` reports is the only one before a write.
    let again = create_at_5().await;
    assert!(
        Arc::ptr_eq(&again, &held),
        "the held handle was not handed back, so this is not the cache hit under test"
    );
    assert_eq!(
        recorder.fences(),
        vec![OplogFence {
            agent_id: agent_id.clone(),
            expected_epoch: ShardEpoch(5),
            actual_epoch: Some(ShardEpoch(6)),
            writer_conflict: false,
        }]
    );
}

#[test]
async fn a_refused_append_reports_the_stored_epoch_and_the_latch_does_not_report_again(
    _tracing: &Tracing,
) {
    let tempdir = tempfile::TempDir::new().unwrap();
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "appended-by-the-loser".into(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let recorder = Arc::new(RecordingFenceObserver::default());
    let owning_executor = fencing_oplog_service(&tempdir, "shared").await;
    let losing_executor = fencing_oplog_service(&tempdir, "shared")
        .await
        .with_fence_observer(recorder.clone());
    let open = |service: &PrimaryOplogService, epoch: u64| {
        let service = service.clone();
        let agent_id = agent_id.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .open(
                    &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
                    &owned_agent_id,
                    AgentMode::Durable,
                    None,
                    make_agent_metadata(agent_id, account_id, environment_id),
                    default_last_known_status(),
                    default_execution_status(AgentMode::Durable),
                    Some(ShardEpoch(epoch)),
                )
                .await
        }
    };

    let stale = open(&losing_executor, 5).await;
    assert!(stale.fence().is_none());
    assert!(recorder.fences().is_empty());
    let owner = open(&owning_executor, 6).await;

    let write = async {
        stale.add(OplogEntry::exited().rounded()).await?;
        stale.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    };
    let refused = write.await;
    assert!(
        matches!(refused, Err(OplogError::Fenced(_))),
        "expected the losing executor's write to be fenced, got {refused:?}"
    );
    let expected = OplogFence {
        agent_id: agent_id.clone(),
        expected_epoch: ShardEpoch(5),
        actual_epoch: Some(ShardEpoch(6)),
        writer_conflict: false,
    };
    assert_eq!(
        recorder.fences(),
        vec![expected.clone()],
        "one refused append is one report"
    );

    let again = async {
        stale.add(OplogEntry::exited().rounded()).await?;
        stale.commit(CommitLevel::Always).await?;
        Ok::<_, OplogError>(())
    }
    .await;
    assert!(matches!(again, Err(OplogError::Fenced(_))));
    assert_eq!(
        recorder.fences(),
        vec![expected],
        "the latched fast-fail asked the storage nothing, so it must report nothing"
    );

    // The owner, with no observer, writes as before.
    owner.add(OplogEntry::suspend().rounded()).await.unwrap();
    owner.commit(CommitLevel::Always).await.unwrap();
}

async fn open_unfenced_fork_target(
    service: &MultiLayerOplogService,
    owned_agent_id: &OwnedAgentId,
) -> Arc<dyn Oplog> {
    service
        .open(
            &mut service.lock_lifecycle(&owned_agent_id.agent_id).await,
            owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(
                owned_agent_id.agent_id.clone(),
                AccountId::new(),
                owned_agent_id.environment_id,
            ),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
}

#[test]
async fn aborting_a_transfer_waits_for_the_prefix_drop_it_handed_to_the_primary(
    _tracing: &Tracing,
) {
    let (drop_prefix_started_tx, drop_prefix_started_rx) = oneshot::channel();
    let release_drop_prefix = Arc::new(Notify::new());
    let primary_storage = Arc::new(ReadCountingIndexedStorage::blocking_drop_prefix(
        drop_prefix_started_tx,
        release_drop_prefix.clone(),
    ));
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            primary_storage.clone(),
            blob_storage.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let service = MultiLayerOplogService::new(
        primary.clone(),
        nev![
            Arc::new(CompressedOplogArchiveService::new(
                Arc::new(InMemoryIndexedStorage::new()),
                1,
                RetryConfig::default(),
            )) as Arc<dyn OplogArchiveService>,
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
                as Arc<dyn OplogArchiveService>
        ],
        2,
        1,
    );
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "fork-target-prefix-drop".to_string(),
        },
    );
    let target = open_unfenced_fork_target(&service, &owned_agent_id).await;

    for _ in 0..3 {
        target.add(OplogEntry::no_op(None).rounded()).await.unwrap();
    }
    target.commit(CommitLevel::Always).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), drop_prefix_started_rx)
        .await
        .expect("the transfer did not reach the primary's prefix drop")
        .expect("prefix drop start signal dropped");

    // The transfer is waiting for the primary's actor, which is inside the prefix drop.
    let abort = tokio::spawn({
        let target = target.clone();
        async move { MultiLayerOplog::try_abort_transfer(&target).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !abort.is_finished(),
        "the abort returned while the primary was still dropping the transferred prefix"
    );

    release_drop_prefix.notify_one();
    tokio::time::timeout(Duration::from_secs(1), abort)
        .await
        .expect("the abort did not return once the prefix drop finished")
        .unwrap();
    assert!(
        primary
            .read_source(&owned_agent_id, AgentMode::Durable, OplogIndex::INITIAL, 3)
            .await
            .is_empty(),
        "the abort returned before the primary finished dropping the transferred prefix"
    );
}

/// `try_abort_transfer` can land between `append_target` and `drop_source_prefix` (see
/// `BackgroundTransfer::run`'s doc comment): the chunk this test appends models one that already
/// reached the archive when that happened. The real owner's next transfer would start from the
/// same, never-trimmed source range and derive the identical chunk id and bytes - exercised here
/// directly against the archive rather than by racing a real abort, since the archive is what
/// must tolerate the repeat.
#[test]
async fn compressed_archive_append_reconciles_a_resumed_transfers_repeat_chunk(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let archive_service =
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1, RetryConfig::default());
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "resumed-transfer".into(),
        },
    );
    let archive = archive_service
        .open_fresh(&owned_agent_id, AgentMode::Durable)
        .await;

    let chunk = vec![
        (OplogIndex::from_u64(1), OplogEntry::suspend().rounded()),
        (OplogIndex::from_u64(2), OplogEntry::exited().rounded()),
    ];
    archive.append(&chunk).await;
    assert_eq!(archive.length().await, 1);

    // The resumed transfer's repeat: identical id, identical bytes.
    archive.append(&chunk).await;
    assert_eq!(
        archive.length().await,
        1,
        "a resumed transfer's identical repeat chunk must not duplicate"
    );

    // A different chunk landing at the same id is not explainable as a replay and must stay
    // fatal rather than being papered over.
    let different_chunk = vec![
        (OplogIndex::from_u64(1), OplogEntry::suspend().rounded()),
        (OplogIndex::from_u64(2), OplogEntry::suspend().rounded()),
    ];
    assert_panics(archive.append(&different_chunk)).await;
}
