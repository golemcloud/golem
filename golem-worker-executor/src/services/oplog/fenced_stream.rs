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

use crate::metrics::oplog::record_oplog_storage_retry;
use crate::services::oplog::{
    OplogError, OplogFence, PrimaryOplogService, decode_scan_cursor, next_scan_cursor,
    record_epoch_verdict, record_owning_epoch, refuse_if_fenced, retry_scan_storage_op,
};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace,
};
use desert_rust::{BinaryDeserializer, BinarySerializer};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, OwnedAgentId, RetryConfig, ScanCursor, ShardEpoch};
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::{Arc, OnceLock};
use tracing::warn;

/// Runs a storage operation under the retry policy, retrying transient failures. Any other failure,
/// a fence included, is returned for the caller to classify.
async fn retry_storage_op<T, F, Fut>(
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
                    return Err(IndexedStorageError::Transient(format!(
                        "operation '{op_name}' failed for key '{key}' after {attempts} attempts: {msg}"
                    )));
                }
            }
            Err(err) => return Err(err),
        }
    }
}

/// How an archive level's storage calls are labelled and its failures described.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamLabels {
    /// The service label of every storage call.
    pub svc: &'static str,
    /// The entity label of the values the stream holds.
    pub entity: &'static str,
    /// The prefix of the retry operation names.
    pub op: &'static str,
    /// What the stream holds, as failure messages name it.
    pub what: &'static str,
}

/// One agent's entries at one archive level in indexed storage, each keyed by the last oplog
/// index it covers, written as the owner of a shard epoch: the epoch is recorded on the key when
/// the stream is opened and asserted on every write, and the first refusal latches, as it does for
/// the primary oplog. A compressed level keeps its chunks here; a blob level keeps the manifest of
/// its chunks.
///
/// A refused write is `OplogError::Fenced` and is never retried. Any other storage failure is
/// returned as a maintenance failure, which archive maintenance retries later.
#[derive(Debug)]
pub(crate) struct FencedIndexedStream {
    agent_id: AgentId,
    namespace: IndexedStorageNamespace,
    key: String,
    labels: StreamLabels,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: RetryConfig,
    /// The writer generation every write asserts. `None` asserts nothing.
    shard_epoch: Option<ShardEpoch>,
    /// Set by the first refused write, or at open when a newer owner's generation was already
    /// recorded. Every later write through this stream is refused without reaching the storage.
    fence: OnceLock<OplogFence>,
}

impl FencedIndexedStream {
    /// A stream that asserts no epoch, for reads, deletion and executors without a shard
    /// assignment.
    pub fn new(
        agent_id: AgentId,
        namespace: IndexedStorageNamespace,
        labels: StreamLabels,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        retry_config: RetryConfig,
    ) -> Self {
        let key = agent_id.to_redis_key();
        Self {
            agent_id,
            namespace,
            key,
            labels,
            indexed_storage,
            retry_config,
            shard_epoch: None,
            fence: OnceLock::new(),
        }
    }

    /// A stream that writes as the owner of `shard_epoch`: records it on the key (a monotonic
    /// compare-and-set, as the primary oplog does at open) and asserts it on every write. A newer
    /// owner's record refuses the open, and the stream is returned already fenced.
    pub async fn opened(
        agent_id: AgentId,
        namespace: IndexedStorageNamespace,
        labels: StreamLabels,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        retry_config: RetryConfig,
        shard_epoch: Option<ShardEpoch>,
    ) -> Self {
        let mut stream = Self::new(agent_id, namespace, labels, indexed_storage, retry_config);
        let Some(shard_epoch) = shard_epoch else {
            return stream;
        };
        stream.shard_epoch = Some(shard_epoch);
        if let Some(fence) = record_owning_epoch(
            &*stream.indexed_storage,
            &stream.retry_config,
            stream.namespace.clone(),
            &stream.agent_id,
            &stream.key,
            shard_epoch,
        )
        .await
        {
            let _ = stream.fence.set(fence);
        }
        stream
    }

    pub fn fence(&self) -> Option<OplogFence> {
        self.fence.get().cloned()
    }

    /// Where every write starts: refused, without reaching the storage, once the stream is fenced.
    pub fn refuse_if_fenced(&self) -> Result<(), OplogError> {
        refuse_if_fenced(self.fence.get())
    }

    /// Appends `value` under `id`, asserting the stream's epoch.
    pub async fn append<V: BinarySerializer + Sync>(
        &self,
        id: OplogIndex,
        value: &V,
    ) -> Result<(), OplogError> {
        let (appended, _) = self.try_append(id, value).await;
        appended.map_err(|error| self.write_error("append", error))
    }

    /// Appends like [`Self::append`], and returns `Ok(false)` when the storage answered that `id`
    /// is already held. Nothing was stored then, and nothing of this append is still on its way.
    /// The answer is passed on only when the first attempt received it: an attempt repeated after
    /// a transient failure may follow one that was sent, so after a repeat it is a failure like
    /// any other. Only a namespace whose storage reports [`IndexedStorageError::Conflict`] gives
    /// the answer.
    pub async fn append_unless_held<V: BinarySerializer + Sync>(
        &self,
        id: OplogIndex,
        value: &V,
    ) -> Result<bool, OplogError> {
        match self.try_append(id, value).await {
            (Ok(()), _) => Ok(true),
            (Err(IndexedStorageError::Conflict(_)), 1) => Ok(false),
            (Err(error), _) => Err(self.write_error("append", error)),
        }
    }

    /// The outcome of an append under the retry policy, and how many attempts it took.
    async fn try_append<V: BinarySerializer + Sync>(
        &self,
        id: OplogIndex,
        value: &V,
    ) -> (Result<(), IndexedStorageError>, u32) {
        let id: u64 = id.into();
        let labels = self.labels;
        let shard_epoch = self.shard_epoch;
        let mut attempts = 0u32;
        let appended = retry_storage_op(
            &self.retry_config,
            &format!("{}_append", labels.op),
            &self.key,
            || {
                attempts += 1;
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move {
                    is.with_entity(labels.svc, "append", labels.entity)
                        .append(ns, &key, id, value, shard_epoch)
                        .await
                }
            },
        )
        .await;
        if shard_epoch.is_some() {
            record_epoch_verdict("archive_append", &appended);
        }
        (appended, attempts)
    }

    /// The first entry whose id is at least `id`.
    pub async fn closest<V: BinaryDeserializer>(
        &self,
        id: OplogIndex,
    ) -> Result<Option<(OplogIndex, V)>, IndexedStorageError> {
        Ok(self
            .indexed_storage
            .with_entity(self.labels.svc, "read", self.labels.entity)
            .closest::<V>(self.namespace.clone(), &self.key, id.into())
            .await?
            .map(|(id, value)| (OplogIndex::from_u64(id), value)))
    }

    /// Every entry whose id lies in `from..=to`.
    pub async fn read<V: BinaryDeserializer>(
        &self,
        from: OplogIndex,
        to: OplogIndex,
    ) -> Result<Vec<(OplogIndex, V)>, String> {
        let labels = self.labels;
        retry_storage_op(
            &self.retry_config,
            &format!("{}_read", labels.op),
            &self.key,
            || {
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move {
                    is.with_entity(labels.svc, "read", labels.entity)
                        .read::<V>(ns, &key, from.into(), to.into())
                        .await
                }
            },
        )
        .await
        .map(|entries| {
            entries
                .into_iter()
                .map(|(id, value)| (OplogIndex::from_u64(id), value))
                .collect()
        })
        .map_err(|error| self.read_error("read", error))
    }

    pub async fn length(&self) -> Result<u64, String> {
        let labels = self.labels;
        retry_storage_op(
            &self.retry_config,
            &format!("{}_length", labels.op),
            &self.key,
            || {
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move { is.with(labels.svc, "length").length(ns, &key).await }
            },
        )
        .await
        .map_err(|error| self.read_error("read the length of", error))
    }

    /// The id of the last entry, or `OplogIndex::NONE` when there is none.
    pub async fn last_id(&self) -> Result<OplogIndex, String> {
        let labels = self.labels;
        retry_storage_op(
            &self.retry_config,
            &format!("{}_last_id", labels.op),
            &self.key,
            || {
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move {
                    is.with_entity(labels.svc, "last_id", labels.entity)
                        .last_id(ns, &key)
                        .await
                }
            },
        )
        .await
        .map(|id| OplogIndex::from_u64(id.unwrap_or_default()))
        .map_err(|error| self.read_error("read the last index of", error))
    }

    pub async fn exists(&self) -> Result<bool, String> {
        let labels = self.labels;
        retry_storage_op(
            &self.retry_config,
            &format!("{}_exists", labels.op),
            &self.key,
            || {
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move { is.with(labels.svc, "exists").exists(ns, &key).await }
            },
        )
        .await
        .map_err(|error| self.read_error("check the existence of", error))
    }

    /// Trims every entry up to `last_dropped_id`, asserting the stream's epoch, and deletes the
    /// key once it holds nothing: only while it is still empty, and the writer generation stays
    /// behind, so a newer owner's or a concurrent writer's entries are never removed and this
    /// owner can keep writing the key. Returns how many entries remain. A refusal removes nothing.
    pub async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError> {
        let labels = self.labels;
        let dropped_id: u64 = last_dropped_id.into();
        let shard_epoch = self.shard_epoch;
        let trimmed = retry_storage_op(
            &self.retry_config,
            &format!("{}_drop_prefix", labels.op),
            &self.key,
            || {
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move {
                    is.with(labels.svc, "drop_prefix")
                        .drop_prefix(ns, &key, dropped_id, shard_epoch)
                        .await
                }
            },
        )
        .await;
        if shard_epoch.is_some() {
            record_epoch_verdict("archive_drop_prefix", &trimmed);
        }
        if let Err(error) = trimmed {
            return Err(self.write_error("drop the prefix of", error));
        }
        let remaining = self.length().await.map_err(OplogError::Maintenance)?;
        if remaining == 0 {
            let deleted = retry_storage_op(
                &self.retry_config,
                &format!("{}_delete_empty", labels.op),
                &self.key,
                || {
                    let is = self.indexed_storage.clone();
                    let ns = self.namespace.clone();
                    let key = self.key.clone();
                    async move {
                        is.with(labels.svc, "drop_prefix")
                            .delete_empty_with_epoch(ns, &key, shard_epoch)
                            .await
                    }
                },
            )
            .await;
            if shard_epoch.is_some() {
                record_epoch_verdict("archive_delete_empty", &deleted);
            }
            match deleted {
                Err(error @ IndexedStorageError::Fenced { .. }) => return Err(self.latch(error)),
                Err(error) => warn!(
                    agent_id = %self.agent_id,
                    namespace = ?self.namespace,
                    error = %error,
                    "Failed to remove an emptied oplog archive key after deleting its entries"
                ),
                Ok(_) => {}
            }
        }
        Ok(remaining)
    }

    /// Deletes the key and its recorded generation, whatever epoch recorded it. For deleting the
    /// agent, which runs after the fenced delete of its primary oplog.
    pub async fn delete(&self) -> Result<(), String> {
        let labels = self.labels;
        retry_storage_op(
            &self.retry_config,
            &format!("{}_delete", labels.op),
            &self.key,
            || {
                let is = self.indexed_storage.clone();
                let ns = self.namespace.clone();
                let key = self.key.clone();
                async move {
                    is.with(labels.svc, "delete")
                        .delete_with_epoch(ns, &key, None)
                        .await
                }
            },
        )
        .await
        .map_err(|error| self.read_error("delete", error))
    }

    fn read_error(&self, action: &str, error: IndexedStorageError) -> String {
        format!(
            "failed to {action} {} for {}: {error}",
            self.labels.what, self.agent_id
        )
    }

    /// Classifies a failed storage write: a refusal latches the fence and ends this stream's
    /// writes, and anything else is a maintenance failure the archive transfer retries later.
    fn write_error(&self, action: &str, error: IndexedStorageError) -> OplogError {
        match error {
            IndexedStorageError::Fenced { .. } => self.latch(error),
            other => OplogError::Maintenance(self.read_error(action, other)),
        }
    }

    /// Latches the fence a refused storage call reported, and returns it as the write's error.
    fn latch(&self, error: IndexedStorageError) -> OplogError {
        let fence = OplogFence::refused(self.agent_id.clone(), error);
        if self.fence.set(fence.clone()).is_ok() {
            warn!(
                agent_id = %self.agent_id,
                namespace = ?self.namespace,
                expected_epoch = fence.expected_epoch.0,
                actual_epoch = ?fence.actual_epoch.map(|epoch| epoch.0),
                "Oplog archive write fenced: the shard has a new owner, refusing further writes"
            );
        }
        OplogError::Fenced(fence)
    }
}

/// A page of the agents of one component that hold a stream in `meta(mode)`, for the archive
/// services' `scan_for_component`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan_component_streams(
    indexed_storage: &Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: &RetryConfig,
    labels: StreamLabels,
    environment_id: &EnvironmentId,
    component_id: &ComponentId,
    modes: Option<AgentMode>,
    cursor: ScanCursor,
    count: u64,
    meta: impl Fn(AgentMode) -> IndexedStorageMetaNamespace,
) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
    let state = decode_scan_cursor(&cursor, modes)?;
    let namespace = meta(state.mode);
    let prefix = PrimaryOplogService::key_prefix(component_id);
    let resume = state.resume.clone();
    let (next_resume, keys) = retry_scan_storage_op(
        retry_config,
        &format!("{}_scan", labels.op),
        &prefix,
        || {
            let is = indexed_storage.clone();
            let namespace = namespace.clone();
            let prefix = prefix.clone();
            let resume = resume.clone();
            async move {
                is.with(labels.svc, "scan")
                    .scan_stable(namespace, Some(&prefix), resume, count)
                    .await
            }
        },
    )
    .await?;

    let next_cursor = next_scan_cursor(state, modes, next_resume)?;
    Ok((
        next_cursor,
        keys.into_iter()
            .map(|key| OwnedAgentId {
                agent_id: PrimaryOplogService::get_agent_id_from_key(&key, component_id),
                environment_id: *environment_id,
            })
            .collect(),
    ))
}
