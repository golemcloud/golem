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

use crate::metrics::oplog::{record_oplog_call, record_oplog_storage_retry};
use crate::metrics::storage::{
    STORAGE_TYPE_OPLOG, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::model::ExecutionStatus;
use crate::services::oplog::reader::{
    OplogReadSource, checked_range_end, exact_from_source, fail_stop,
};
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, OpenOplogs, Oplog,
    OplogAddReceipt, OplogCloseCompletion, OplogConstructor, OplogError, OplogFence,
    OplogFenceObserver, OplogLifecycleGuard, OplogService, OrderedOplogStart, PendingUpload,
    ReservedPayload, ReservedRawStartBuilder, cursor_value, next_scan_cursor, scan_modes,
};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::FutureExt;
use golem_common::model::RetryConfig;
use golem_common::model::ShardEpoch;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
use golem_common::model::{
    AgentId, AgentMetadata, AgentStatusRecord, DurableStreamSessionStatus, OwnedAgentId, ScanCursor,
};
use golem_common::read_only_lock;
use golem_common::retries::get_delay;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::cmp::{max, min};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, warn};

/// Runs a storage operation under the retry policy, panicking on anything it cannot retry away.
///
/// Reads, deletions and prefix drops keep this shape: a permanent failure there is a broken
/// deployment, and failing fast is the long-standing contract.
async fn retry_storage_op<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    key: &str,
    op: F,
) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    match retry_storage_op_fenceable(retry_config, op_name, key, op).await {
        Ok(val) => val,
        Err(err) => panic!("Indexed storage operation '{op_name}' failed for key '{key}': {err}"),
    }
}

/// As [`retry_storage_op`], but hands a fence back instead of panicking on it.
///
/// A fenced write is not a storage failure: the storage is healthy and refused the write on
/// purpose, because this executor no longer owns the agent's shard. Retrying cannot change that,
/// and panicking would take the whole executor down over one agent that simply moved. Every other
/// permanent failure still panics, so the fail-stop contract is unchanged for everything else -
/// including the primary-key collision that has always been the crude fence.
async fn retry_storage_op_fenceable<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    key: &str,
    mut op: F,
) -> Result<T, IndexedStorageError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(val) => return Ok(val),
            Err(err @ IndexedStorageError::Fenced { .. }) => return Err(err),
            Err(IndexedStorageError::Transient(msg)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    record_oplog_storage_retry(op_name);
                    warn!(
                        op = op_name,
                        key = key,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error, retrying: {msg}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: Transient storage error: {msg}"
                    );
                }
            }
            Err(err) => {
                panic!("Indexed storage operation '{op_name}' failed for key '{key}': {err}");
            }
        }
    }
}

fn stored_batch_matches(mut actual: Vec<(u64, Vec<u8>)>, expected: &[(u64, Bytes)]) -> bool {
    actual.sort_unstable_by_key(|(id, _)| *id);
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(
            |((actual_id, actual_value), (expected_id, expected_value))| {
                actual_id == expected_id && actual_value.as_slice() == expected_value.as_ref()
            },
        )
}

async fn stored_oplog_batch_matches(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    key: &str,
    expected: &[(u64, Bytes)],
) -> Option<bool> {
    let first_id = expected.first().expect("non-empty oplog batch").0;
    let last_id = expected.last().expect("non-empty oplog batch").0;
    retry_storage_op(retry_config, "append_reconcile", key, || {
        let namespace = namespace.clone();
        async move {
            indexed_storage
                .with_entity("oplog", "append_reconcile", "entry")
                .read_raw(namespace, key, first_id, last_id)
                .await
                .map(|actual| {
                    if actual.is_empty() {
                        None
                    } else {
                        Some(stored_batch_matches(actual, expected))
                    }
                })
        }
    })
    .await
}

enum SerializedOplogAppend {
    Entry((u64, Bytes)),
    Batch(Arc<[(u64, Bytes)]>),
}

impl SerializedOplogAppend {
    fn entries(&self) -> &[(u64, Bytes)] {
        match self {
            Self::Entry(entry) => std::slice::from_ref(entry),
            Self::Batch(entries) => entries,
        }
    }

    async fn write(
        &self,
        indexed_storage: &(dyn IndexedStorage + Send + Sync),
        namespace: &IndexedStorageNamespace,
        api_name: &'static str,
        key: &str,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let storage = indexed_storage.with_entity("oplog", api_name, "entry");
        match self {
            Self::Entry((id, value)) => {
                storage
                    .append_raw(namespace.clone(), key, *id, value.to_vec(), shard_epoch)
                    .await
            }
            Self::Batch(entries) => {
                storage
                    .append_many_raw(namespace, key, entries.clone(), shard_epoch)
                    .await
            }
        }
    }
}

async fn retry_oplog_append(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    op_name: &str,
    api_name: &'static str,
    key: &str,
    append: SerializedOplogAppend,
    shard_epoch: Option<ShardEpoch>,
) -> Result<(), IndexedStorageError> {
    let mut attempts = 0u32;
    let mut write_may_have_committed = false;
    loop {
        attempts += 1;
        let error = match append
            .write(indexed_storage, namespace, api_name, key, shard_epoch)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        // The storage refused the write because the shard has a new owner. Not a transient
        // failure to retry and not an indeterminate one to reconcile: it is a deliberate refusal,
        // so hand it back and let the caller give up this one agent instead of aborting.
        if matches!(error, IndexedStorageError::Fenced { .. }) {
            return Err(error);
        }

        let retryable = match &error {
            IndexedStorageError::Indeterminate(_) => {
                write_may_have_committed = true;
                true
            }
            IndexedStorageError::Transient(_) => true,
            IndexedStorageError::Conflict(msg) => {
                if !write_may_have_committed {
                    panic!(
                        "Indexed storage operation '{op_name}' conflicted for key '{key}' without a preceding indeterminate write; possible concurrent oplog writer: {msg}"
                    );
                }
                false
            }
            IndexedStorageError::Other(_) => {
                if !write_may_have_committed {
                    panic!("Indexed storage operation '{op_name}' failed for key '{key}': {error}");
                }
                false
            }
            // Returned above; named here only because the guard does not make this exhaustive.
            IndexedStorageError::Fenced { .. } => unreachable!("a fence returns before this match"),
        };

        if write_may_have_committed {
            match stored_oplog_batch_matches(
                retry_config,
                indexed_storage,
                namespace,
                key,
                append.entries(),
            )
            .await
            {
                Some(true) => return Ok(()),
                Some(false) => {
                    // The stored content differs from what this attempt sent - the only
                    // legitimate way that happens is a new owner having already written those
                    // same indices. Repeat the write once as a probe: the backends check the
                    // epoch inside the same transaction as the insert, so a shard that has
                    // moved on is fenced before the insert is even attempted. A same-epoch
                    // conflict instead fails the probe's insert (still fatal, below) - the
                    // mismatch is unexplained and not safe to paper over.
                    if let Some(epoch) = shard_epoch
                        && let Err(fenced @ IndexedStorageError::Fenced { .. }) = append
                            .write(indexed_storage, namespace, api_name, key, Some(epoch))
                            .await
                    {
                        return Err(fenced);
                    }
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' and the indeterminate write did not match storage: {error}"
                    )
                }
                None => {}
            }
        }

        if retryable && let Some(delay) = get_delay(retry_config, attempts) {
            record_oplog_storage_retry(op_name);
            warn!(
                op = op_name,
                key = key,
                attempt = attempts,
                delay_ms = delay.as_millis() as u64,
                error = %error,
                "Retryable indexed storage write failed, retrying"
            );
            tokio::time::sleep(delay).await;
            continue;
        }

        if write_may_have_committed {
            panic!(
                "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts and the indeterminate write did not match storage: {error}"
            );
        } else {
            panic!(
                "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: {error}"
            );
        }
    }
}

/// Records the epoch this executor is allowed to write `key` with, and reports the fence when
/// the stored record is already ahead of it.
///
/// Monotonic on the storage side once a record exists, so a re-grant at a higher epoch takes the
/// oplog over while an executor holding a stale one cannot claim it back. An oplog with no record
/// (new, deleted, or from before the record existed) is claimed by whichever epoch opens it
/// first. Written before the oplog's first entry -
/// an absent record fences too, which is what closes the window between creating an oplog and
/// recording who owns it.
///
/// A refusal is also handed to `fence_observer`, because the epoch it carries is what a shard
/// manager whose state lost history has to mint above.
async fn record_owning_epoch(
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    retry_config: &RetryConfig,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    key: &str,
    shard_epoch: ShardEpoch,
    fence_observer: Option<&dyn OplogFenceObserver>,
) -> Option<OplogFence> {
    let outcome = retry_storage_op_fenceable(retry_config, "upsert_oplog_metadata", key, || {
        let ns = IndexedStorageNamespace::OpLog {
            agent_id: owned_agent_id.agent_id(),
            agent_mode,
        };
        async move {
            indexed_storage
                .upsert_oplog_metadata("oplog", "upsert_oplog_metadata", ns, key, shard_epoch)
                .await
        }
    })
    .await;

    match outcome {
        Ok(()) => None,
        Err(IndexedStorageError::Fenced {
            expected,
            actual,
            owner_conflict,
            ..
        }) => {
            warn!(
                agent_id = %owned_agent_id,
                expected_epoch = expected.0,
                actual_epoch = ?actual.map(|epoch| epoch.0),
                owner_conflict,
                "Oplog opened at a stale shard epoch: the shard has a new owner"
            );
            let fence = OplogFence {
                agent_id: owned_agent_id.agent_id(),
                expected_epoch: expected,
                actual_epoch: actual,
                owner_conflict,
            };
            if let Some(observer) = fence_observer {
                observer.fenced(&fence);
            }
            Some(fence)
        }
        // `retry_storage_op_fenceable` panics on every other permanent failure.
        Err(other) => unreachable!("unexpected storage error: {other}"),
    }
}

async fn read_persisted_oplog_entries(
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    namespace: IndexedStorageNamespace,
    key: String,
    start: u64,
    end: u64,
) -> Result<Vec<(u64, OplogEntry)>, IndexedStorageError> {
    let entries = indexed_storage
        .with_entity("oplog", "read", "entry")
        .read_raw(namespace, &key, start, end)
        .await?;

    tokio::task::spawn_blocking(move || {
        entries
            .into_iter()
            .map(|(idx, bytes)| deserialize(&bytes).map(|entry| (idx, entry)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexedStorageError::Other)
    })
    .await
    .map_err(|error| {
        IndexedStorageError::Other(format!("oplog deserialization task failed: {error}"))
    })?
}

/// The primary oplog service implementation, suitable for direct use (top level of a multi-layered setup).
///
/// Stores and retrieves individual oplog entries from the `IndexedStorage` implementation configured for
/// the executor.
#[derive(Clone)]
pub struct PrimaryOplogService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_operations_before_commit_ephemeral: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    oplogs: OpenOplogs,
    stream_session_index: Arc<std::sync::OnceLock<Arc<super::StreamSessionIndexService>>>,
    /// Told of every refusal the storage returns for an oplog this service opened, so the epochs
    /// the refusals carry can reach the shard manager. `None` reports nothing.
    fence_observer: Option<Arc<dyn OplogFenceObserver>>,
}

impl Debug for PrimaryOplogService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrimaryOplogService")
            .field("indexed_storage", &self.indexed_storage)
            .field("blob_storage", &self.blob_storage)
            .field("replicas", &self.replicas)
            .field(
                "max_operations_before_commit",
                &self.max_operations_before_commit,
            )
            .field(
                "max_operations_before_commit_ephemeral",
                &self.max_operations_before_commit_ephemeral,
            )
            .field("max_payload_size", &self.max_payload_size)
            .field("retry_config", &self.retry_config)
            .field("oplogs", &self.oplogs)
            .field("stream_session_index", &self.stream_session_index)
            .field("fence_observer", &self.fence_observer.is_some())
            .finish()
    }
}

impl PrimaryOplogService {
    pub async fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_operations_before_commit: u64,
        max_operations_before_commit_ephemeral: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
    ) -> Self {
        let replicas = retry_storage_op(&retry_config, "number_of_replicas", "global", || {
            let is = indexed_storage.clone();
            async move { is.with("oplog", "new").number_of_replicas().await }
        })
        .await;
        Self {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_operations_before_commit_ephemeral,
            max_payload_size,
            retry_config,
            oplogs: OpenOplogs::new("primary oplog"),
            stream_session_index: Arc::new(std::sync::OnceLock::new()),
            fence_observer: None,
        }
    }

    /// Reports every refusal the storage returns for an oplog this service opens to `observer`,
    /// with the epoch recorded on the oplog.
    pub fn with_fence_observer(mut self, observer: Arc<dyn OplogFenceObserver>) -> Self {
        self.fence_observer = Some(observer);
        self
    }

    fn oplog_key(agent_id: &AgentId) -> String {
        agent_id.to_redis_key()
    }

    pub fn key_prefix(component_id: &ComponentId) -> String {
        component_id.0.to_string()
    }

    async fn append_initial_entry(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        op_name: &str,
        api_name: &'static str,
        entry: &OplogEntry,
        shard_epoch: Option<ShardEpoch>,
    ) {
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let namespace = IndexedStorageNamespace::OpLog {
            agent_id: owned_agent_id.agent_id(),
            agent_mode,
        };
        let value = Bytes::from(
            serialize(entry)
                .unwrap_or_else(|err| panic!("Failed to serialize initial oplog entry: {err}")),
        );

        retry_oplog_append(
            &self.retry_config,
            self.indexed_storage.as_ref(),
            &namespace,
            op_name,
            api_name,
            &key,
            SerializedOplogAppend::Entry((1, value)),
            shard_epoch,
        )
        .await
        .unwrap_or_else(|err| {
            // Only a fence reaches here - every other permanent failure already panicked inside
            // `retry_oplog_append`. Nothing more is written: `open` records the epoch again and,
            // being refused there too, hands back an oplog that refuses every write.
            warn!(
                agent_id = %owned_agent_id,
                error = %err,
                "Initial oplog entry fenced: the shard has a new owner"
            );
        });
    }

    async fn open_with(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        shard_epoch: Option<ShardEpoch>,
        reconcile_last_index: bool,
    ) -> Arc<dyn Oplog> {
        record_oplog_call("open");

        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let max_operations_before_commit = match agent_mode {
            AgentMode::Durable => self.max_operations_before_commit,
            AgentMode::Ephemeral => self.max_operations_before_commit_ephemeral,
        };

        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    shard_epoch,
                    self.indexed_storage.clone(),
                    self.blob_storage.clone(),
                    self.replicas,
                    max_operations_before_commit,
                    self.max_payload_size,
                    self.retry_config.clone(),
                    key,
                    last_oplog_index,
                    reconcile_last_index,
                    owned_agent_id.clone(),
                    agent_mode,
                    initial_worker_metadata.created_by,
                    self.stream_session_index(),
                    self.fence_observer.clone(),
                ),
            )
            .await
    }

    async fn get_last_index_from_storage(
        indexed_storage: &(dyn IndexedStorage + Send + Sync),
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        retry_config: &RetryConfig,
    ) -> OplogIndex {
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let agent_id = owned_agent_id.agent_id();
        OplogIndex::from_u64(
            retry_storage_op(retry_config, "get_last_index", &key, || {
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    indexed_storage
                        .with_entity("oplog", "get_last_index", "entry")
                        .last_id(ns, &key)
                        .await
                }
            })
            .await
            .unwrap_or_default(),
        )
    }

    pub fn get_agent_id_from_key(key: &str, component_id: &ComponentId) -> AgentId {
        let redis_prefix = format!("{}:", component_id.0);
        if key.starts_with(&redis_prefix) {
            let agent_name = &key[redis_prefix.len()..];
            AgentId {
                agent_id: agent_name.to_string(),
                component_id: *component_id,
            }
        } else {
            panic!("Failed to get worker id from indexed storage key: {key}")
        }
    }

    async fn upload_raw_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_payload_size: usize,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        if data.len() > max_payload_size {
            let payload_id: PayloadId = PayloadId::new();
            let md5_hash = md5::compute(&data).to_vec();

            blob_storage
                .put_raw(
                    "oplog",
                    "upload_payload",
                    BlobStorageNamespace::OplogPayload {
                        environment_id: owned_agent_id.environment_id(),
                        agent_id: owned_agent_id.agent_id(),
                        agent_mode,
                    },
                    Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
                    &data,
                )
                .await
                .map_err(|e| format!("Failed uploading oplog data to the blob store {e}"))?;

            Ok(RawOplogPayload::External {
                payload_id,
                md5_hash,
            })
        } else {
            Ok(RawOplogPayload::SerializedInline(data))
        }
    }

    async fn download_raw_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        blob_storage
                    .get_raw(
                        "oplog",
                        "download_payload",
                        BlobStorageNamespace::OplogPayload {
                            environment_id: owned_agent_id.environment_id(),
                            agent_id: owned_agent_id.agent_id(),
                            agent_mode,
                        },
                        Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
                    )
                    .await
                    .map_err(|e| format!("Failed downloading oplog data from the blob store {e}"))?
                    .ok_or(format!("Payload not found (worker: {owned_agent_id}, payload_id: {payload_id}, md5 hash: {md5_hash:02X?})"))
    }
}

#[async_trait]
impl OplogService for PrimaryOplogService {
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        self.oplogs.lock_lifecycle(agent_id).await
    }

    fn set_stream_session_index(&self, index: Arc<super::StreamSessionIndexService>) {
        assert!(
            self.stream_session_index.set(index).is_ok(),
            "stream session index is already installed"
        );
    }

    fn stream_session_index(&self) -> Option<Arc<super::StreamSessionIndexService>> {
        self.stream_session_index.get().cloned()
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Arc<dyn Oplog> {
        record_oplog_call("create");
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        let key = Self::oplog_key(&owned_agent_id.agent_id);

        // The record goes in before the existence probe and the first entry. A probe taken before
        // the claim can miss a `Create` that an executor at an older epoch lands in between, and
        // the initial append would then collide with it. If the claim is refused, this executor
        // has already lost the shard: it writes nothing, so whether the owner created the oplog
        // first is not its question, and `open` below hands back an oplog that refuses every write.
        // The refusal is reported here even though the open behind it may report it again: an
        // unfenced handle this service still holds at the same epoch is handed back without
        // asking the storage, and then this is the only refusal before a write.
        let fenced_at_create = match shard_epoch {
            Some(epoch) => record_owning_epoch(
                &*self.indexed_storage,
                &self.retry_config,
                owned_agent_id,
                agent_mode,
                &key,
                epoch,
                self.fence_observer.as_deref(),
            )
            .await
            .is_some(),
            None => false,
        };

        if !fenced_at_create {
            let already_exists: bool = {
                let is = self.indexed_storage.clone();
                let agent_id = owned_agent_id.agent_id();
                let key = key.clone();
                retry_storage_op(&self.retry_config, "create_exists", &key, || {
                    let is = is.clone();
                    let ns = IndexedStorageNamespace::OpLog {
                        agent_id: agent_id.clone(),
                        agent_mode,
                    };
                    let key = key.clone();
                    async move { is.with("oplog", "create").exists(ns, &key).await }
                })
                .await
            };

            if already_exists {
                panic!("oplog for worker {owned_agent_id} already exists in indexed storage")
            }

            self.append_initial_entry(
                owned_agent_id,
                agent_mode,
                "create_append",
                "create",
                &initial_entry,
                shard_epoch,
            )
            .await;
        }

        // The claim came before the initial entry, so `INITIAL` is exact and needs no re-read.
        self.open_with(
            lifecycle,
            owned_agent_id,
            agent_mode,
            Some(OplogIndex::INITIAL),
            initial_worker_metadata,
            shard_epoch,
            false,
        )
        .await
    }

    async fn create_fresh(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Arc<dyn Oplog> {
        record_oplog_call("create_fresh");
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        // The caller guarantees the agent id is freshly derived and unused, so
        // the existence probe performed by `create` is skipped: the initial
        // entry is appended directly without any prior read. The epoch record still goes in
        // first - a fresh agent id does not mean a fresh shard.
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let fenced_at_create = match shard_epoch {
            Some(epoch) => record_owning_epoch(
                &*self.indexed_storage,
                &self.retry_config,
                owned_agent_id,
                agent_mode,
                &key,
                epoch,
                self.fence_observer.as_deref(),
            )
            .await
            .is_some(),
            None => false,
        };

        if !fenced_at_create {
            self.append_initial_entry(
                owned_agent_id,
                agent_mode,
                "create_fresh_append",
                "create_fresh",
                &initial_entry,
                shard_epoch,
            )
            .await;
        }

        // Claimed before the initial entry, so `INITIAL` is exact; not re-reading it keeps a fresh
        // create free of storage reads.
        self.open_with(
            lifecycle,
            owned_agent_id,
            agent_mode,
            Some(OplogIndex::INITIAL),
            initial_worker_metadata,
            shard_epoch,
            false,
        )
        .await
    }

    async fn open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Arc<dyn Oplog> {
        // An index handed in by a caller was read before this open claims the epoch.
        let reconcile_last_index = last_oplog_index.is_some();
        self.open_with(
            lifecycle,
            owned_agent_id,
            agent_mode,
            last_oplog_index,
            initial_worker_metadata,
            shard_epoch,
            reconcile_last_index,
        )
        .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
        record_oplog_call("get_last_index");
        Self::get_last_index_from_storage(
            &*self.indexed_storage,
            owned_agent_id,
            agent_mode,
            &self.retry_config,
        )
        .await
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) {
        record_oplog_call("delete");
        lifecycle.assert_agent(&owned_agent_id.agent_id);

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            // The epoch record goes before the entries: a writer still holding this oplog open is
            // then refused by the absent record, instead of appending entries back into an oplog
            // that is being removed.
            retry_storage_op(&self.retry_config, "delete_oplog_metadata", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    is.delete_oplog_metadata("oplog", "delete_oplog_metadata", ns, &key)
                        .await
                }
            })
            .await;
            retry_storage_op(&self.retry_config, "delete", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "delete").delete(ns, &key).await }
            })
            .await;
        }
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        if n == 0 {
            return BTreeMap::new();
        }
        let entries = self.read_source(owned_agent_id, agent_mode, idx, n).await;
        fail_stop(exact_from_source(OplogReadSource::Primary, idx, n, entries))
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read");
        let Some(end) = fail_stop(checked_range_end(idx, n)) else {
            return BTreeMap::new();
        };

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            let start: u64 = idx.into();
            let end: u64 = end.into();
            retry_storage_op(&self.retry_config, "read", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { read_persisted_oplog_entries(is, ns, key, start, end).await }
            })
            .await
            .into_iter()
            .map(|(k, v): (u64, OplogEntry)| (OplogIndex::from_u64(k), v))
            .collect()
        }
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
        record_oplog_call("exists");

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            retry_storage_op(&self.retry_config, "exists", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "exists").exists(ns, &key).await }
            })
            .await
        }
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        record_oplog_call("scan");

        let (active_mode, next_mode) = scan_modes(modes, cursor.cursor);
        let cursor_val = cursor_value(cursor.cursor);

        let (next_cursor_val, keys) = {
            let is = self.indexed_storage.clone();
            let prefix = Self::key_prefix(component_id);
            retry_storage_op(&self.retry_config, "scan", &prefix, || {
                let is = is.clone();
                let prefix = prefix.clone();
                async move {
                    is.with("oplog", "scan")
                        .scan(
                            IndexedStorageMetaNamespace::Oplog {
                                agent_mode: active_mode,
                            },
                            Some(&prefix),
                            cursor_val,
                            count,
                        )
                        .await
                }
            })
            .await
        };

        let next_cursor = next_scan_cursor(next_cursor_val, active_mode, next_mode, cursor.layer);
        let owned_agent_ids = keys
            .into_iter()
            .map(|key| OwnedAgentId {
                agent_id: Self::get_agent_id_from_key(&key, component_id),
                environment_id: *environment_id,
            })
            .collect();

        Ok((next_cursor, owned_agent_ids))
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        Self::upload_raw_payload(
            self.blob_storage.clone(),
            self.max_payload_size,
            owned_agent_id,
            agent_mode,
            data,
        )
        .await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        Self::download_raw_payload(
            self.blob_storage.clone(),
            owned_agent_id,
            agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    key: String,
    last_oplog_idx: Option<OplogIndex>,
    /// `last_oplog_idx` was read before this constructor claims the epoch, so it may be behind
    /// entries an executor at an older epoch committed in between.
    reconcile_last_index: bool,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    stream_session_index: Option<Arc<super::StreamSessionIndexService>>,
    shard_epoch: Option<ShardEpoch>,
    fence_observer: Option<Arc<dyn OplogFenceObserver>>,
}

impl CreateOplogConstructor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        shard_epoch: Option<ShardEpoch>,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
        key: String,
        last_oplog_idx: Option<OplogIndex>,
        reconcile_last_index: bool,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        stream_session_index: Option<Arc<super::StreamSessionIndexService>>,
        fence_observer: Option<Arc<dyn OplogFenceObserver>>,
    ) -> Self {
        Self {
            shard_epoch,
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_payload_size,
            retry_config,
            key,
            last_oplog_idx,
            reconcile_last_index,
            owned_agent_id,
            agent_mode,
            account_id,
            stream_session_index,
            fence_observer,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.shard_epoch
    }

    async fn create_oplog(
        self,
        _lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog> {
        // Recorded before the oplog is usable, so an executor whose shard has moved is refused
        // at its very first write rather than after replaying the new owner's entries.
        let fence = match self.shard_epoch {
            Some(shard_epoch) => {
                record_owning_epoch(
                    &*self.indexed_storage,
                    &self.retry_config,
                    &self.owned_agent_id,
                    self.agent_mode,
                    &self.key,
                    shard_epoch,
                    self.fence_observer.as_deref(),
                )
                .await
            }
            None => None,
        };

        // Read after the claim: once it returns, every writer at an older epoch is refused, so the
        // read sees every entry that will ever precede this handle's first write. An index a
        // caller read before the claim can be behind by whatever a losing executor committed in
        // between, and this handle's first append would collide with it. That index is merged
        // rather than replaced, because it may count entries this layer no longer holds, such as
        // ones already moved to an archive.
        let stored_last_index = || {
            PrimaryOplogService::get_last_index_from_storage(
                &*self.indexed_storage,
                &self.owned_agent_id,
                self.agent_mode,
                &self.retry_config,
            )
        };
        let last_oplog_idx = match self.last_oplog_idx {
            None => stored_last_index().await,
            Some(idx)
                if self.reconcile_last_index && self.shard_epoch.is_some() && fence.is_none() =>
            {
                OplogIndex::from_u64(idx.as_u64().max(stored_last_index().await.as_u64()))
            }
            Some(idx) => idx,
        };

        Arc::new(PrimaryOplog::new(
            self.shard_epoch,
            fence,
            self.indexed_storage,
            self.blob_storage,
            self.replicas,
            self.max_operations_before_commit,
            self.max_payload_size,
            self.retry_config,
            self.key,
            last_oplog_idx,
            self.owned_agent_id,
            self.agent_mode,
            self.account_id,
            self.stream_session_index,
            self.fence_observer,
            close,
        ))
    }
}

/// The primary oplog behind an actor boundary.
///
/// A dedicated tokio task (the actor) exclusively owns [`PrimaryOplogState`]; every `Oplog`
/// method sends an [`OplogJob`] over an unbounded channel and awaits the job's oneshot reply.
/// There is deliberately **no shared lock** between callers.
///
/// This shape is required for deadlock freedom, not just style. `Oplog` methods are awaited from
/// two kinds of callers that must never block each other through shared ownership:
///
/// * futures polled by wasmtime's store event loop (concurrent p3 durable host calls), and
/// * host code running on wasm fibers that suspend while *keeping the store* (async libcalls such
///   as the `memory.grow` resource limiter, and non-concurrent `wrap_async`-style host functions
///   like the p2 stdio streams).
///
/// While such a fiber is suspended, the event loop cannot poll any store-polled future
/// (wasmtime's documented store-blocking limitation, wasmtime#11869/#11870). With a shared lock —
/// even a lock whose critical sections are purely synchronous — a store-polled future that is
/// queued on the lock becomes its next owner on FIFO handoff and then cannot run until the event
/// loop polls it, so a fiber queued behind it deadlocks the whole store. With the actor, callers
/// only await oneshot completions, which never make an unpolled caller the owner of anything, so
/// fiber-side waits always make progress as long as the actor task (polled directly by tokio)
/// does.
///
/// The actor must therefore never await anything that is produced by a store event loop; it only
/// performs storage I/O.
///
/// ORDERING (Start determinism): `add_start_with_reserved_raw_payload` relies on jobs being
/// processed in the order they were enqueued. `mpsc::UnboundedSender::send` is synchronous, and
/// callers enqueue as their first non-awaiting step, so enqueue order equals the order in which
/// concurrent durable calls initiated their operation — the same guarantee the previous
/// FIFO-fair mutex provided via `lock()` acquisition order.
struct PrimaryOplog {
    jobs: tokio::sync::mpsc::UnboundedSender<OplogJob>,
    closed: OplogCloseCompletion,
    tasks: super::WorkerTasks,
    retired: AtomicBool,
    key: String,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    /// The epoch the actor's state asserts on every append, copied here so that reading it does
    /// not have to go through the actor. Fixed for the oplog's lifetime.
    shard_epoch: Option<ShardEpoch>,
    /// The refusal the actor's state has latched, shared so the handle can report it.
    fence: Arc<std::sync::OnceLock<OplogFence>>,
    stream_session_index: Option<Arc<super::StreamSessionIndexService>>,
    close: Mutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

/// A request processed by the [`PrimaryOplog`] actor task, which exclusively owns the oplog
/// state. Mutating jobs that grow the buffer past the commit threshold run the resulting commit
/// inside the actor before replying, preserving the pre-actor behavior where `add` blocked the
/// caller on a threshold-triggered commit.
enum OplogJob {
    Close,
    Add {
        entry: OplogEntry,
        done: tokio::sync::oneshot::Sender<Result<OplogIndex, OplogError>>,
    },
    AddDurableStreamBatch {
        make_batch: DurableStreamBatchBuilder,
        done: tokio::sync::oneshot::Sender<Result<Vec<(OplogIndex, OplogEntry)>, OplogError>>,
    },
    AddPair {
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        done: tokio::sync::oneshot::Sender<Result<(OplogIndex, OplogIndex), OplogError>>,
    },
    AddStart {
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, OplogError>>,
    },
    AddIndexedStart {
        build_request: IndexedReservedStartBuilder,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, OplogError>>,
    },
    Commit {
        level: CommitLevel,
        done: tokio::sync::oneshot::Sender<Result<BTreeMap<OplogIndex, OplogEntry>, OplogError>>,
    },
    Flush {
        done: tokio::sync::oneshot::Sender<()>,
    },
    DropPrefix {
        last_dropped_id: OplogIndex,
        done: tokio::sync::oneshot::Sender<u64>,
    },
    CurrentIndex {
        done: tokio::sync::oneshot::Sender<OplogIndex>,
    },
    RawDurableStreamSessionStatus {
        session_key: golem_common::model::durable_stream::StreamSessionKey,
        done: tokio::sync::oneshot::Sender<RawSessionLookup>,
    },
    CompleteRawDurableStreamSessionStatus {
        session_key: golem_common::model::durable_stream::StreamSessionKey,
        expected_watermark: OplogIndex,
        expected_committed: OplogIndex,
        status: Result<Option<DurableStreamSessionStatus>, String>,
        done: tokio::sync::oneshot::Sender<Option<super::RawDurableStreamSessionStatus>>,
    },
    LastAddedNonHintEntry {
        done: tokio::sync::oneshot::Sender<Option<OplogIndex>>,
    },
    /// Snapshots the state needed for reads and replica waits; the caller performs the storage
    /// I/O itself, off the actor, so large reads do not head-of-line block writes.
    Reader {
        done: tokio::sync::oneshot::Sender<OplogReader>,
    },
    /// Snapshots the state needed for payload blob uploads/downloads.
    BlobContext {
        done: tokio::sync::oneshot::Sender<OplogBlobContext>,
    },
}

struct RawSessionLookup {
    watermark: OplogIndex,
    committed: OplogIndex,
    buffer: VecDeque<OplogEntry>,
    cached: Option<Result<Option<DurableStreamSessionStatus>, String>>,
}

/// Snapshot of the state needed to upload/download oplog payload blobs outside the actor.
struct OplogBlobContext {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    max_payload_size: usize,
}

impl Drop for PrimaryOplog {
    fn drop(&mut self) {
        let _ = self.jobs.send(OplogJob::Close);
        if let Some(close) = self.close.get_mut().unwrap().take() {
            close();
        }
    }
}

impl PrimaryOplog {
    #[allow(clippy::too_many_arguments)]
    fn new(
        shard_epoch: Option<ShardEpoch>,
        fence: Option<OplogFence>,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
        key: String,
        last_oplog_idx: OplogIndex,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        stream_session_index: Option<Arc<super::StreamSessionIndexService>>,
        fence_observer: Option<Arc<dyn OplogFenceObserver>>,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let account_id_label = account_id.to_string();
        let environment_id_label = owned_agent_id.environment_id().to_string();
        let fence = Arc::new(match fence {
            Some(fence) => std::sync::OnceLock::from(fence),
            None => std::sync::OnceLock::new(),
        });
        let mut state = PrimaryOplogState {
            shard_epoch,
            fence: fence.clone(),
            fence_observer,
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_payload_size,
            retry_config,
            key: key.clone(),
            buffer: VecDeque::new(),
            last_committed_idx: last_oplog_idx,
            last_reported_commit_idx: last_oplog_idx,
            last_oplog_idx,
            owned_agent_id,
            agent_mode,
            account_id,
            account_id_label,
            environment_id_label,
            last_added_non_hint_entry: None,
            pending_uploads: Vec::new(),
            durable_stream_sessions: super::raw_session::RawSessionCache::default(),
        };
        let owned_agent_id = state.owned_agent_id.clone();
        let agent_mode = state.agent_mode;

        let (jobs, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<OplogJob>();
        let actor = tokio::spawn(async move {
            while let Some(job) = job_rx.recv().await {
                match job {
                    OplogJob::Close => break,
                    OplogJob::Add { entry, done } => {
                        record_oplog_call("add");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let idx = state.push(entry);
                        // A threshold commit failing must fail the `add` that triggered it: the
                        // caller would otherwise be told its entry landed when the batch it was
                        // folded into was refused.
                        let result = match state.maybe_commit().await {
                            Ok(()) => Ok(idx),
                            Err(error) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::AddDurableStreamBatch { make_batch, done } => {
                        record_oplog_call("add_durable_stream_batch");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let first_index = state.last_oplog_idx.next();
                        let records = make_batch(first_index);
                        let serialized = records
                            .into_iter()
                            .map(|record| record.serialize().map(|bytes| (record, bytes)))
                            .collect::<Result<Vec<_>, _>>();
                        let result = serialized.and_then(|serialized| {
                            let mut prepared = Vec::with_capacity(serialized.len());
                            for (record, bytes) in serialized {
                                let ReservedPayload {
                                    raw,
                                    pending: _,
                                    guard,
                                } = state.reserve_raw_payload(bytes);
                                prepared.push((record.into_entry(raw)?, guard));
                            }
                            let mut result = Vec::with_capacity(prepared.len());
                            for (entry, guard) in prepared {
                                let index = state.push(entry.clone());
                                drop(guard);
                                result.push((index, entry));
                            }
                            Ok(result)
                        });
                        let result = match (result, state.maybe_commit().await) {
                            (Ok(value), Ok(())) => Ok(value),
                            (Err(error), _) => Err(error.into()),
                            (Ok(_), Err(error)) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::AddPair {
                        start,
                        make_second,
                        done,
                    } => {
                        record_oplog_call("add_pair");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let first_idx = state.push(start);
                        let second = make_second(first_idx);
                        let second_idx = state.push(second);
                        // Both halves of the pair share the threshold commit, so a refused
                        // commit fails the pair rather than reporting a write that was rolled
                        // back.
                        let result = state.maybe_commit().await.map(|()| (first_idx, second_idx));
                        let _ = done.send(result);
                    }
                    OplogJob::AddStart {
                        serialized_request,
                        build_start,
                        done,
                    } => {
                        record_oplog_call("add_start_with_reserved_raw_payload");
                        // ORDERING (Start determinism) — CRITICAL SECTION: reserving the payload,
                        // building the `Start`, and assigning its index happen as one non-yielding
                        // step here on the actor, so a concurrently enqueued writer cannot
                        // interleave its own `Start` and reorder the deterministic replay
                        // sequence. The reservation only *starts* the (possibly large) blob
                        // upload; it is not awaited here. Durability of the blob before any
                        // referencing entry is committed is enforced by the commit barrier in
                        // `append`.
                        //
                        // The no-`.await` window is enforced at compile time by the `!Send`
                        // `guard`: this actor future must stay `Send` for `tokio::spawn`, so a
                        // refactor holding the guard across an `.await` is rejected rather than
                        // silently breaking ordering. Do not move `drop(guard)` before `push`.
                        //
                        // A fenced oplog refuses before reserving, so no upload is started for a
                        // `Start` that can never be written.
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let result = {
                            let ReservedPayload {
                                raw,
                                pending,
                                guard,
                            } = state.reserve_raw_payload(serialized_request);
                            match build_start(raw) {
                                Ok(entry) => {
                                    let index = state.push(entry.clone());
                                    drop(guard);
                                    Ok(OrderedOplogStart {
                                        index,
                                        entry,
                                        pending_upload: pending,
                                    })
                                }
                                Err(err) => Err(err),
                            }
                        };
                        let result = match (result, state.maybe_commit().await) {
                            (Ok(value), Ok(())) => Ok(value),
                            (Err(error), _) => Err(error.into()),
                            (Ok(_), Err(error)) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::AddIndexedStart {
                        build_request,
                        done,
                    } => {
                        record_oplog_call("add_start_with_indexed_reserved_raw_payload");
                        if let Err(error) = state.refuse_if_fenced() {
                            let _ = done.send(Err(error));
                            continue;
                        }
                        let result = build_request(state.last_oplog_idx.next()).and_then(
                            |(serialized_request, build_start)| {
                                let ReservedPayload {
                                    raw,
                                    pending,
                                    guard,
                                } = state.reserve_raw_payload(serialized_request);
                                let entry = build_start(raw)?;
                                let index = state.push(entry.clone());
                                drop(guard);
                                Ok(OrderedOplogStart {
                                    index,
                                    entry,
                                    pending_upload: pending,
                                })
                            },
                        );
                        let result = match (result, state.maybe_commit().await) {
                            (Ok(value), Ok(())) => Ok(value),
                            (Err(error), _) => Err(error.into()),
                            (Ok(_), Err(error)) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::Commit { level, done } => {
                        let previously_committed_through = state.last_committed_idx;
                        let result = match state.commit(level).await {
                            Ok(committed) => Ok(state
                                .committed_since_last_report(
                                    previously_committed_through,
                                    committed,
                                )
                                .await),
                            Err(error) => Err(error),
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::Flush { done } => {
                        // The job has no error to reply with. A fence is latched on the state,
                        // where `wait_for_replicas` reads it after this job, so a fenced flush is
                        // reported as not durable rather than lost; every subsequent write fails on
                        // it without asking the storage again. A transient storage failure is fatal
                        // here as everywhere else.
                        match state.commit(CommitLevel::Always).await {
                            Ok(_) | Err(OplogError::Fenced(_)) => {}
                            Err(error) => panic!("oplog write: {error}"),
                        }
                        let _ = done.send(());
                    }
                    OplogJob::DropPrefix {
                        last_dropped_id,
                        done,
                    } => {
                        let before = state.reader().length().await;
                        state.drop_prefix(last_dropped_id).await;
                        let remaining = state.reader().length().await;
                        if remaining == 0 {
                            state.delete().await;
                        }
                        let dropped = before - remaining;
                        if dropped > 0 {
                            let account_id = state.account_id.to_string();
                            let environment_id = state.owned_agent_id.environment_id().to_string();
                            record_storage_objects_deleted(
                                STORAGE_TYPE_OPLOG,
                                &account_id,
                                &environment_id,
                                dropped,
                            );
                        }
                        let _ = done.send(dropped);
                    }
                    OplogJob::CurrentIndex { done } => {
                        let _ = done.send(state.last_oplog_idx);
                    }
                    OplogJob::RawDurableStreamSessionStatus { session_key, done } => {
                        let cached = state
                            .durable_stream_sessions
                            .cached(&session_key)
                            .transpose();
                        let _ = done.send(RawSessionLookup {
                            watermark: state.last_oplog_idx,
                            committed: state.last_committed_idx,
                            buffer: if cached.is_none() {
                                state.buffer.clone()
                            } else {
                                VecDeque::new()
                            },
                            cached,
                        });
                    }
                    OplogJob::CompleteRawDurableStreamSessionStatus {
                        session_key,
                        expected_watermark,
                        expected_committed,
                        status,
                        done,
                    } => {
                        let result = if state.last_oplog_idx == expected_watermark
                            && state.last_committed_idx == expected_committed
                        {
                            if let Ok(value) = &status {
                                state
                                    .durable_stream_sessions
                                    .insert(session_key, value.clone());
                            }
                            Some(super::RawDurableStreamSessionStatus {
                                watermark: expected_watermark,
                                status,
                            })
                        } else {
                            None
                        };
                        let _ = done.send(result);
                    }
                    OplogJob::LastAddedNonHintEntry { done } => {
                        let _ = done.send(state.last_added_non_hint_entry);
                    }
                    OplogJob::Reader { done } => {
                        let _ = done.send(state.reader());
                    }
                    OplogJob::BlobContext { done } => {
                        let _ = done.send(OplogBlobContext {
                            blob_storage: state.blob_storage.clone(),
                            owned_agent_id: state.owned_agent_id.clone(),
                            agent_mode: state.agent_mode,
                            account_id: state.account_id,
                            max_payload_size: state.max_payload_size,
                        });
                    }
                }
            }
            let mut upload_result = Ok(());
            for upload in state.pending_uploads {
                upload_result = upload_result.and(upload.wait().await);
            }
            upload_result
        });

        Self {
            jobs,
            closed: async move { actor.await.map_err(|error| error.to_string())? }
                .boxed()
                .shared(),
            tasks: super::WorkerTasks::default(),
            retired: AtomicBool::new(false),
            key,
            owned_agent_id,
            agent_mode,
            shard_epoch,
            fence,
            stream_session_index,
            close: Mutex::new(Some(close)),
        }
    }

    /// Sends a job to the actor and waits for its reply.
    ///
    /// A missing reply means the actor failed or this handle was used after retirement.
    /// Orderly shutdown drains jobs queued before Close.
    async fn run_job<R>(
        &self,
        make_job: impl FnOnce(tokio::sync::oneshot::Sender<R>) -> OplogJob,
    ) -> R {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(make_job(done)).is_err() {
            panic!("Oplog actor for {} terminated unexpectedly", self.key);
        }
        match done_rx.await {
            Ok(result) => result,
            Err(_) => panic!(
                "Oplog actor for {} dropped a request without replying",
                self.key
            ),
        }
    }
}

/// A snapshot of [`PrimaryOplogState`] sufficient to serve reads. The actor hands this snapshot
/// to the caller, which performs the read I/O itself, off the actor task — so large reads never
/// head-of-line block writes (see [`OplogJob::Reader`]). The buffer snapshot keeps both single and
/// batched reads' visibility of not-yet-committed entries.
struct OplogReader {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: RetryConfig,
    key: String,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    last_committed_idx: OplogIndex,
    buffer: VecDeque<OplogEntry>,
    replicas: u8,
}

impl OplogReader {
    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        record_oplog_call("read");

        let buffer_start = self.last_committed_idx.next();
        if oplog_index >= buffer_start {
            let offset = u64::from(oplog_index).saturating_sub(u64::from(buffer_start));
            if let Ok(offset) = usize::try_from(offset)
                && let Some(entry) = self.buffer.get(offset)
            {
                return entry.clone();
            }
        }

        let entries: Vec<(u64, OplogEntry)> = {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            let idx: u64 = oplog_index.into();
            retry_storage_op(&self.retry_config, "read", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { read_persisted_oplog_entries(is, ns, key, idx, idx).await }
            })
            .await
        };

        entries
            .into_iter()
            .next()
            .map(|(_, entry)| entry)
            .unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry {oplog_index} for {} in indexed storage",
                    self.key
                )
            })
    }

    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read_exact");

        let Some(last_idx) = fail_stop(checked_range_end(oplog_index, n)) else {
            return BTreeMap::new();
        };

        let mut result: BTreeMap<OplogIndex, OplogEntry> = if oplog_index <= self.last_committed_idx
        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            let start: u64 = oplog_index.into();
            let end: u64 = min(last_idx, self.last_committed_idx).into();
            retry_storage_op(&self.retry_config, "read_exact", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { read_persisted_oplog_entries(is, ns, key, start, end).await }
            })
            .await
            .into_iter()
            .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry))
            .collect()
        } else {
            BTreeMap::new()
        };

        if last_idx >= self.last_committed_idx {
            // There can be some uncommitted entries in the buffer
            if !self.buffer.is_empty() {
                let requested_start: u64 = oplog_index.into();
                let requested_end: u64 = last_idx.into();
                let buffer_start: u64 = self.last_committed_idx.next().into();
                let buffer_end = buffer_start + self.buffer.len() as u64 - 1;
                let overlap_start = max(requested_start, buffer_start);
                let overlap_end = min(requested_end, buffer_end);

                if overlap_start <= overlap_end {
                    let offset = (overlap_start - buffer_start) as usize;
                    let count = (overlap_end - overlap_start + 1) as usize;
                    for idx in 0..count {
                        result.insert(
                            OplogIndex::from_u64(overlap_start + idx as u64),
                            self.buffer[offset + idx].clone(),
                        );
                    }
                }
            }
        }

        result
    }

    async fn length(&self) -> u64 {
        record_oplog_call("length");

        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "length", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "length").length(ns, &key).await }
            })
            .await
        }
    }
}

struct PrimaryOplogState {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    last_reported_commit_idx: OplogIndex,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    account_id_label: String,
    environment_id_label: String,
    last_added_non_hint_entry: Option<OplogIndex>,
    /// In-flight external payload uploads started by [`PrimaryOplogState::reserve_raw_payload`] but
    /// not yet known to be durable. The commit barrier in `append` waits on these before persisting
    /// any buffered entries, so no committed entry can reference a not-yet-written blob.
    pending_uploads: Vec<PendingUpload>,
    durable_stream_sessions: super::raw_session::RawSessionCache,
    /// The shard epoch this executor held for the agent's shard when the oplog was opened, and
    /// the one every append asserts.
    ///
    /// Cached at open rather than read per write: one live oplog is one ownership generation, and
    /// this is the value its metadata row was written with. A renewal never changes it - an epoch
    /// only moves when the shard changes owner, and then this oplog is the losing side.
    shard_epoch: Option<ShardEpoch>,
    /// Set once a write has been refused, or at open when the epoch record already belonged to a
    /// newer owner. Every later write fails on it immediately: the oplog is another executor's
    /// now, so there is nothing to be gained by asking the storage again. Shared with the handle,
    /// which answers [`Oplog::fence`] from it without a round trip through the actor.
    fence: Arc<std::sync::OnceLock<OplogFence>>,
    /// Told of the refusal that sets [`Self::fence`]; the latched fast-fail asks the storage
    /// nothing and reports nothing.
    fence_observer: Option<Arc<dyn OplogFenceObserver>>,
}

impl PrimaryOplogState {
    /// Computes a payload reference and, for an external (large) payload, spawns its blob upload and
    /// registers it for the commit barrier — **without** awaiting the upload.
    ///
    /// Intentionally a non-`async` `fn`: computing the reference, spawning the upload, and
    /// registering the [`PendingUpload`] happen as one non-yielding step inside the actor's
    /// `AddStart` job handler, which then builds and `push`es the `Start` from this reference with
    /// no `.await` in between, so concurrent calls' `Start` entries stay in initiation order. That
    /// no-`.await` window is enforced at compile time by the returned `!Send` [`ReserveGuard`].
    fn reserve_raw_payload(&mut self, data: Vec<u8>) -> ReservedPayload {
        if data.len() > self.max_payload_size {
            let payload_id = PayloadId::new();
            let md5_hash = md5::compute(&data).to_vec();
            let path = format!("{}/{}", hex::encode(&md5_hash), payload_id.0);
            let data_len = data.len() as u64;

            let blob_storage = self.blob_storage.clone();
            let environment_id = self.owned_agent_id.environment_id();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let account_id = self.account_id.to_string();
            let environment_id_label = environment_id.to_string();

            let upload = async move {
                blob_storage
                    .put_raw(
                        "oplog",
                        "upload_payload",
                        BlobStorageNamespace::OplogPayload {
                            environment_id,
                            agent_id,
                            agent_mode,
                        },
                        Path::new(&path),
                        &data,
                    )
                    .await
                    .map_err(|e| format!("Failed uploading oplog data to the blob store {e}"))?;
                record_storage_bytes_written(
                    STORAGE_TYPE_OPLOG,
                    &account_id,
                    &environment_id_label,
                    data_len,
                );
                Ok::<(), String>(())
            };

            let upload = tokio::spawn(upload)
                .map(|joined| {
                    joined.unwrap_or_else(|join_err| {
                        Err(format!("oplog payload upload task failed: {join_err}"))
                    })
                })
                .boxed()
                .shared();
            let pending = PendingUpload::spawned(upload);
            self.pending_uploads.push(pending.clone());
            ReservedPayload::new(
                RawOplogPayload::External {
                    payload_id,
                    md5_hash,
                },
                pending,
            )
        } else {
            ReservedPayload::new(
                RawOplogPayload::SerializedInline(data),
                PendingUpload::already_durable(),
            )
        }
    }

    async fn append(
        &mut self,
        entries: Vec<OplogEntry>,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        record_oplog_call("append");

        // Already refused once: fail fast rather than re-asking the storage. Only entries buffered
        // before the fence latched can reach here, and they go back where they were.
        if let Some(fence) = self.fence.get() {
            let fence = fence.clone();
            self.retain_refused(entries);
            return Err(OplogError::Fenced(fence));
        }

        // Commit barrier: every deferred external payload reserved during this session must be
        // durably written to blob storage before the entries (which may reference it) are persisted
        // to indexed storage. `append` flushes the whole buffer, so waiting on all outstanding
        // uploads is correct. A permanent upload failure is treated like a permanent storage
        // failure (see `retry_storage_op`): there is no safe way to commit a dangling reference.
        if !self.pending_uploads.is_empty() {
            let pending = std::mem::take(&mut self.pending_uploads);
            let mut result = Ok(());
            for upload in pending {
                result = result.and(upload.wait().await);
            }
            if let Err(err) = result {
                panic!(
                    "Oplog payload upload failed for key '{}', cannot commit referencing entries: {err}",
                    self.key
                );
            }
        }

        if entries.is_empty() {
            return Ok(BTreeMap::new());
        }

        let entry_count = entries.len() as u64;
        let mut pairs = Vec::with_capacity(entries.len());
        let mut last_idx = self.last_committed_idx;
        for entry in entries {
            let oplog_idx = last_idx.next();
            pairs.push((oplog_idx.into(), entry));
            last_idx = oplog_idx;
        }
        let mut bytes_written = 0u64;
        let mut serialized_pairs = Vec::with_capacity(pairs.len());
        for (id, entry) in &pairs {
            let value = serialize(entry)
                .unwrap_or_else(|err| panic!("Failed to serialize oplog entry: {err}"));
            bytes_written += value.len() as u64;
            serialized_pairs.push((*id, Bytes::from(value)));
        }
        let serialized_pairs: Arc<[(u64, Bytes)]> = serialized_pairs.into();
        let namespace = IndexedStorageNamespace::OpLog {
            agent_id: self.owned_agent_id.agent_id(),
            agent_mode: self.agent_mode,
        };
        let appended = retry_oplog_append(
            &self.retry_config,
            self.indexed_storage.as_ref(),
            &namespace,
            "append",
            "append",
            &self.key,
            SerializedOplogAppend::Batch(serialized_pairs),
            self.shard_epoch,
        )
        .await
        .map_err(|err| Self::as_oplog_error(&self.owned_agent_id, err));
        if let Err(error) = appended {
            if let OplogError::Fenced(fence) = &error {
                // The commit barrier above already awaited every payload the batch referenced, so
                // those blobs are durable and stay behind with no stored entry pointing at them.
                // The epoch the storage holds is reported, so a shard manager whose state lost
                // history can mint above it. Warned only when it latches: each write after that
                // fails fast on the latch without reaching the storage.
                if self.fence.set(fence.clone()).is_ok() {
                    warn!(
                        agent_id = %self.owned_agent_id,
                        expected_epoch = fence.expected_epoch.0,
                        actual_epoch = ?fence.actual_epoch.map(|epoch| epoch.0),
                        "Oplog append fenced: the shard has a new owner, refusing further writes"
                    );
                }
                if let Some(observer) = &self.fence_observer {
                    observer.fenced(fence);
                }
                self.retain_refused(pairs.into_iter().map(|(_, entry)| entry));
            }
            return Err(error);
        }

        record_storage_bytes_written(
            STORAGE_TYPE_OPLOG,
            &self.account_id_label,
            &self.environment_id_label,
            bytes_written,
        );
        record_storage_objects_written(
            STORAGE_TYPE_OPLOG,
            &self.account_id_label,
            &self.environment_id_label,
            entry_count,
        );

        self.last_committed_idx = last_idx;
        Ok(BTreeMap::from_iter(
            pairs
                .into_iter()
                .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry)),
        ))
    }

    /// Refuses a new write once the fence has latched, before anything is buffered or reserved.
    ///
    /// Without it an add below the commit threshold would only buffer, answer with an index, and
    /// report a write that can never reach the storage.
    fn refuse_if_fenced(&self) -> Result<(), OplogError> {
        match self.fence.get() {
            Some(fence) => Err(OplogError::Fenced(fence.clone())),
            None => Ok(()),
        }
    }

    /// Puts entries a fenced append turned away back at the head of the buffer, where `commit`
    /// drained them from.
    ///
    /// Every index this oplog has handed out stays readable from it: `last_oplog_idx` is not
    /// rolled back, and the reader maps the buffer from `last_committed_idx`. A reader that took
    /// `current_oplog_index` before the refusal and reads after it would otherwise find a gap and
    /// fail-stop the executor. The entries are never sent again, because every later append fails
    /// on the latch, and the buffer cannot grow, because every later add is refused.
    fn retain_refused(&mut self, entries: impl IntoIterator<Item = OplogEntry>) {
        let mut restored: VecDeque<OplogEntry> = entries.into_iter().collect();
        restored.append(&mut self.buffer);
        self.buffer = restored;
    }

    /// Commits if the buffer is over the threshold. Separated out so the actor arms can fold a
    /// threshold-commit failure into the job that triggered it.
    async fn maybe_commit(&mut self) -> Result<(), OplogError> {
        if self.over_commit_threshold() {
            self.commit(CommitLevel::Always).await?;
        }
        Ok(())
    }

    /// Names the agent on a storage error, so the worker that hit it can be given up by id.
    fn as_oplog_error(owned_agent_id: &OwnedAgentId, err: IndexedStorageError) -> OplogError {
        match err {
            IndexedStorageError::Fenced {
                expected,
                actual,
                owner_conflict,
                ..
            } => OplogError::Fenced(OplogFence {
                agent_id: owned_agent_id.agent_id(),
                expected_epoch: expected,
                actual_epoch: actual,
                owner_conflict,
            }),
            other => OplogError::Storage(other.to_string()),
        }
    }

    /// Pushes an entry into the in-memory buffer and advances the oplog index,
    /// without checking the commit threshold. Callers must run [`maybe_commit`]
    /// afterwards. Used by `add_pair` to buffer a `Start`/`End` pair before a
    /// single commit-threshold check, so the pair is never split by a commit.
    fn push(&mut self, entry: OplogEntry) -> OplogIndex {
        let is_hint = entry.is_hint();
        let next_index = self.last_oplog_idx.next();
        self.durable_stream_sessions.apply_entry(next_index, &entry);
        self.buffer.push_back(entry);
        self.last_oplog_idx = next_index;
        if !is_hint {
            self.last_added_non_hint_entry = Some(self.last_oplog_idx);
        }
        self.last_oplog_idx
    }

    /// Snapshots everything needed to serve reads off the actor task, so read I/O never blocks
    /// the actor's job loop (see [`OplogJob::Reader`]).
    fn reader(&self) -> OplogReader {
        OplogReader {
            indexed_storage: self.indexed_storage.clone(),
            retry_config: self.retry_config.clone(),
            key: self.key.clone(),
            owned_agent_id: self.owned_agent_id.clone(),
            agent_mode: self.agent_mode,
            last_committed_idx: self.last_committed_idx,
            buffer: self.buffer.clone(),
            replicas: self.replicas,
        }
    }

    /// Whether the buffer has grown past the commit threshold and a commit should be scheduled.
    /// The commit itself always runs inside the actor, before the triggering job replies (see
    /// the note on [`OplogJob`]).
    fn over_commit_threshold(&self) -> bool {
        self.buffer.len() > self.max_operations_before_commit as usize
    }

    async fn commit(
        &mut self,
        _level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        record_oplog_call("commit");

        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();
        self.append(entries).await
    }

    async fn committed_since_last_report(
        &mut self,
        previously_committed_through: OplogIndex,
        mut newly_committed: BTreeMap<OplogIndex, OplogEntry>,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let committed_through = self.last_committed_idx;
        let mut entries = if self.last_reported_commit_idx < previously_committed_through {
            let start = self.last_reported_commit_idx.next();
            let count =
                u64::from(previously_committed_through) - u64::from(self.last_reported_commit_idx);
            let entries = self.reader().read_source(start, count).await;
            fail_stop(exact_from_source(
                OplogReadSource::Primary,
                start,
                count,
                entries,
            ))
        } else {
            BTreeMap::new()
        };
        entries.append(&mut newly_committed);
        self.last_reported_commit_idx = committed_through;
        entries
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        record_oplog_call("drop_prefix");

        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            let dropped_id: u64 = last_dropped_id.into();
            retry_storage_op(&self.retry_config, "drop_prefix", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    is.with("oplog", "drop_prefix")
                        .drop_prefix(ns, &key, dropped_id)
                        .await
                }
            })
            .await;
        }
    }

    async fn delete(&self) {
        record_oplog_call("delete");

        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "delete", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "delete").delete(ns, &key).await }
            })
            .await;
        }
    }
}

impl Debug for PrimaryOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

#[async_trait]
impl Oplog for PrimaryOplog {
    fn retire(&self) {
        if !self.retired.swap(true, Ordering::AcqRel) {
            let _ = self.jobs.send(OplogJob::Close);
        }
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire) || self.jobs.is_closed()
    }

    fn closed(&self) -> OplogCloseCompletion {
        self.closed.clone()
    }

    fn task_owner(&self) -> Option<&super::WorkerTasks> {
        Some(&self.tasks)
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(OplogJob::Add { entry, done }).is_err() {
            panic!("Oplog actor for {} terminated unexpectedly", self.key);
        }
        let key = self.key.clone();
        Box::pin(async move {
            done_rx.await.unwrap_or_else(|_| {
                panic!("Oplog actor for {key} dropped an add request without replying")
            })
        })
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        self.run_job(|done| OplogJob::AddDurableStreamBatch { make_batch, done })
            .await
    }

    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> Result<(OplogIndex, OplogIndex), OplogError> {
        self.run_job(|done| OplogJob::AddPair {
            start,
            make_second,
            done,
        })
        .await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.run_job(|done| OplogJob::DropPrefix {
            last_dropped_id,
            done,
        })
        .await
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        self.run_job(|done| OplogJob::Commit { level, done }).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.run_job(|done| OplogJob::CurrentIndex { done }).await
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> super::RawDurableStreamSessionStatus {
        loop {
            let snapshot = self
                .run_job(|done| OplogJob::RawDurableStreamSessionStatus {
                    session_key: session_key.clone(),
                    done,
                })
                .await;
            if let Some(status) = snapshot.cached {
                return super::RawDurableStreamSessionStatus {
                    watermark: snapshot.watermark,
                    status,
                };
            }
            let status = super::raw_session::RawSessionCache::reconstruct(
                self.stream_session_index.as_ref(),
                &self.owned_agent_id,
                self.agent_mode,
                snapshot.committed,
                &snapshot.buffer,
                session_key,
            )
            .await;
            if let Some(result) = self
                .run_job(|done| OplogJob::CompleteRawDurableStreamSessionStatus {
                    session_key: session_key.clone(),
                    expected_watermark: snapshot.watermark,
                    expected_committed: snapshot.committed,
                    status,
                    done,
                })
                .await
            {
                return result;
            }
        }
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.run_job(|done| OplogJob::LastAddedNonHintEntry { done })
            .await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        record_oplog_call("wait_for_replicas");

        self.run_job(|done| OplogJob::Flush { done }).await;
        // A refused flush reached no replica. The storage would still answer with its replica
        // count, and passing that on would tell the caller that entries it turned away are durable.
        if self.fence.get().is_some() {
            return false;
        }
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        let replicas = replicas.min(reader.replicas);
        match reader
            .indexed_storage
            .with("oplog", "wait_for_replicas")
            .wait_for_replicas(replicas, timeout)
            .await
        {
            Ok(n) => n == replicas,
            Err(err) => {
                error!("Failed to wait for replicas to sync indexed storage: {err}");
                false
            }
        }
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        let entries = reader.read_source(oplog_index, n).await;
        fail_stop(exact_from_source(
            OplogReadSource::Primary,
            oplog_index,
            n,
            entries,
        ))
    }

    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.read_source(oplog_index, n).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.read(oplog_index).await
    }

    async fn length(&self) -> u64 {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        let data_len = data.len() as u64;
        let result = PrimaryOplogService::upload_raw_payload(
            ctx.blob_storage,
            ctx.max_payload_size,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            data,
        )
        .await;
        if let Ok(RawOplogPayload::External { .. }) = &result {
            // Only count bytes that were actually uploaded externally
            record_storage_bytes_written(
                STORAGE_TYPE_OPLOG,
                &ctx.account_id.to_string(),
                &ctx.owned_agent_id.environment_id().to_string(),
                data_len,
            );
        }
        result
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        PrimaryOplogService::download_raw_payload(
            ctx.blob_storage,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        // ORDERING (Start determinism): the job is enqueued synchronously here — there is no
        // `.await` between a subtask initiating its durable operation and this send — and the
        // actor assigns `Start` indices strictly in job order, so initiation order becomes
        // `Start`-index order exactly as the ordering contract requires. See the ordering note on
        // [`PrimaryOplog`].
        self.run_job(|done| OplogJob::AddStart {
            serialized_request,
            build_start,
            done,
        })
        .await
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.run_job(|done| OplogJob::AddIndexedStart {
            build_request,
            done,
        })
        .await
    }

    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.shard_epoch
    }

    fn fence(&self) -> Option<OplogFence> {
        self.fence.get().cloned()
    }
}
