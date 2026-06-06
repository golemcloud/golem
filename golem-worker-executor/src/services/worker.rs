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

use super::component::ComponentService;
use super::golem_config::GolemConfig;
use super::{HasComponentService, HasConfig, HasOplogService};
use crate::metrics::workers::record_worker_call;
use crate::services::oplog::OplogService;
use crate::services::shard::ShardService;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::status::calculate_last_known_status_for_existing_worker;
use async_trait::async_trait;
use golem_common::model::agent::{AgentMode, LegacyParsedAgentId};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatus, AgentStatusRecord, OwnedAgentId, ShardId,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct GetWorkerMetadataResult {
    // Status of the worker at the time of the create oplog entry
    pub initial_worker_metadata: AgentMetadata,
    // Last known cached status of the worker. Might be outdated
    pub last_known_status: Option<AgentStatusRecord>,
}

/// Service for persisting the current set of Golem workers represented by their metadata
#[async_trait]
pub trait WorkerService: Send + Sync {
    async fn get(&self, owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult>;

    async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult>;

    async fn remove(&self, owned_agent_id: &OwnedAgentId);

    async fn remove_cached_status(&self, owned_agent_id: &OwnedAgentId);

    /// Returns the persisted [`AgentMode`] for the worker, if it exists.
    ///
    /// The mode is decided at worker create time and persisted in the `Create` oplog entry,
    /// and is also kept on the cached [`AgentStatusRecord`]. This method first consults the
    /// cached status record (which exists for active durable workers) and, on cache miss,
    /// probes both oplog namespaces (durable and ephemeral). Returns `None` if no oplog
    /// exists in either namespace.
    async fn get_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentMode>;

    /// Updates the cached status for the worker.
    ///
    /// The `AgentMode` is read from `status_value.agent_mode`. Cached status is only
    /// written for durable workers; for ephemeral workers this is a no-op (their oplog
    /// only exists while they are running, and they keep the status in memory on the
    /// owning executor).
    async fn update_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        status_value: &AgentStatusRecord,
    );
}

#[derive(Clone)]
pub struct DefaultWorkerService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    shard_service: Arc<dyn ShardService>,
    oplog_service: Arc<dyn OplogService>,
    component_service: Arc<dyn ComponentService>,
    config: Arc<GolemConfig>,
}

impl DefaultWorkerService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        oplog_service: Arc<dyn OplogService>,
        component_service: Arc<dyn ComponentService>,
        config: Arc<GolemConfig>,
    ) -> Self {
        Self {
            key_value_storage,
            shard_service,
            oplog_service,
            component_service,
            config,
        }
    }

    async fn enum_workers_at_key(&self, key: &str) -> Vec<GetWorkerMetadataResult> {
        record_worker_call("enum");

        let value: Vec<OwnedAgentId> = self
            .key_value_storage
            .with_entity("worker", "enum", "agent_id")
            .members_of_set(KeyValueStorageNamespace::RunningWorkers, key)
            .await
            .unwrap_or_else(|err| panic!("failed to get worker ids from KV storage: {err}"));

        let mut workers = Vec::new();

        for owned_agent_id in value {
            let metadata = self
                .get(&owned_agent_id)
                .await
                .unwrap_or_else(|| panic!("failed to get worker metadata from KV storage"));
            workers.push(metadata);
        }

        workers
    }

    fn status_key(agent_id: &AgentId) -> String {
        format!("worker:status:{}", agent_id.to_redis_key())
    }

    /// Key holding only the worker's immutable `AgentMode`, stored separately from the status
    /// blob so `get_agent_mode` can resolve the oplog namespace without deserializing the whole
    /// `AgentStatusRecord`. Populated lazily on a `get_agent_mode` cache miss (durable workers
    /// only); never written on the per-commit hot path. The value never changes for the life of
    /// the worker.
    fn agent_mode_key(agent_id: &AgentId) -> String {
        format!("worker:agent_mode:{}", agent_id.to_redis_key())
    }

    fn running_in_shard_key(shard_id: &ShardId) -> String {
        format!("worker:running_in_shard:{shard_id}")
    }

    /// Reads the cached `AgentStatusRecord` for `owned_agent_id`, if any. Returns `None` if
    /// the cache key is missing or the stored value cannot be deserialized in the current format.
    async fn read_cached_status(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentStatusRecord> {
        let status_value: Option<Result<AgentStatusRecord, String>> = self
            .key_value_storage
            .with_entity("worker", "read_cached_status", "worker_status")
            .get_attempt_deserialize(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                &Self::status_key(&owned_agent_id.agent_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get worker status for {owned_agent_id} from KV storage: {err}")
            });

        match status_value {
            Some(Ok(status)) => Some(status),
            // Stored in a previous, no-longer-valid format; treat as cache miss.
            Some(Err(_)) => None,
            None => None,
        }
    }

    /// Reads the dedicated `agent_mode` key, if present. Returns `None` on a cache miss or if the
    /// stored value cannot be deserialized in the current format (treated as a miss).
    async fn read_cached_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
        let value: Option<Result<AgentMode, String>> = self
            .key_value_storage
            .with_entity("worker", "read_cached_agent_mode", "agent_mode")
            .get_attempt_deserialize(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get agent mode for {owned_agent_id} from KV storage: {err}")
            });

        match value {
            Some(Ok(agent_mode)) => Some(agent_mode),
            Some(Err(_)) | None => None,
        }
    }

    /// Populates the dedicated `agent_mode` key. Only called on a `get_agent_mode` cache miss for
    /// durable workers, never on the per-commit hot path. The value is immutable for the life of
    /// the worker, so concurrent writers would write the same value.
    async fn write_cached_agent_mode(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
        self.key_value_storage
            .with_entity("worker", "write_cached_agent_mode", "agent_mode")
            .set(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
                &agent_mode,
            )
            .await
            .unwrap_or_else(|err| panic!("failed to set agent mode in KV storage: {err}"));
    }

    fn should_track_for_assignment_recovery(status: &AgentStatusRecord) -> bool {
        matches!(
            status.status,
            AgentStatus::Running | AgentStatus::Retrying | AgentStatus::Interrupted
        ) || status.has_pending_work()
    }
}

#[async_trait]
impl WorkerService for DefaultWorkerService {
    async fn get(&self, owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult> {
        record_worker_call("get");

        let agent_mode = self.get_agent_mode(owned_agent_id).await?;

        let initial_oplog_entry = self
            .oplog_service
            .read(owned_agent_id, agent_mode, OplogIndex::INITIAL, 1)
            .await
            .into_iter()
            .next();

        debug!("Found initial oplog entry for worker: {initial_oplog_entry:?}");

        match initial_oplog_entry {
            None => None,
            Some((
                _,
                OplogEntry::Create {
                    agent_id,
                    agent_mode: persisted_agent_mode,
                    component_revision,
                    env,
                    environment_id,
                    created_by,
                    timestamp,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins,
                    local_agent_config,
                    original_phantom_id,
                    instance_id,
                },
            )) => {
                debug_assert_eq!(persisted_agent_mode, agent_mode);
                let agent_mode = persisted_agent_mode;
                let agent_type_name =
                    LegacyParsedAgentId::parse_agent_type_name(&agent_id.agent_id).ok();
                let component_metadata = self
                    .component_service
                    .get_metadata(agent_id.component_id, Some(component_revision))
                    .await
                    .map_or_else(
                        |e| match e {
                            WorkerExecutorError::ComponentNotFound { .. } => Ok(None),
                            other => Err(other),
                        },
                        |v| Ok(Some(v)),
                    )
                    .unwrap_or_else(|err| {
                        panic!("failed to get component metadata for {owned_agent_id}: {err}")
                    })?;

                let config = local_agent_config
                    .into_iter()
                    .map(|lac| {
                        lac.enrich_with_type(&component_metadata.metadata, agent_type_name.as_ref())
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap_or_else(|err| {
                        panic!("failed enriching local agent config for {owned_agent_id}: {err}")
                    });

                let initial_worker_metadata = AgentMetadata {
                    agent_id,
                    env,
                    config,
                    environment_id,
                    created_by,
                    created_at: timestamp,
                    parent,
                    last_known_status: AgentStatusRecord {
                        component_revision,
                        component_revision_for_replay: component_revision,
                        component_size,
                        total_linear_memory_size: initial_total_linear_memory_size,
                        active_plugins: initial_active_plugins,
                        agent_mode,
                        ..AgentStatusRecord::default()
                    },
                    original_phantom_id,
                    fingerprint: AgentFingerprint(instance_id),
                    agent_mode,
                };

                let last_known_status = match self.read_cached_status(owned_agent_id).await {
                    Some(mut status) => {
                        // `agent_mode` is `#[transient]` and therefore not part of the status
                        // blob; restore it from the authoritative value resolved above so the
                        // returned record carries the correct mode instead of the `Durable`
                        // deserialization default.
                        status.agent_mode = agent_mode;
                        Some(status)
                    }
                    // No cached status (cache miss, missing for ephemeral workers, or stale
                    // format) -> recompute from oplog.
                    None => {
                        let last_known_status = calculate_last_known_status_for_existing_worker(
                            self,
                            owned_agent_id,
                            agent_mode,
                            None,
                        )
                        .await;

                        self.update_cached_status(owned_agent_id, &last_known_status)
                            .await;

                        Some(last_known_status)
                    }
                };

                Some(GetWorkerMetadataResult {
                    initial_worker_metadata,
                    last_known_status,
                })
            }
            Some(_) => panic!("Encountered malformed oplog without create oplog entry"),
        }
    }

    async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult> {
        let shard_assignment = self.shard_service.try_get_current_assignment();
        let mut result: Vec<GetWorkerMetadataResult> = vec![];
        if let Some(shard_assignment) = shard_assignment {
            for shard_id in shard_assignment.shard_ids {
                let key = Self::running_in_shard_key(&shard_id);
                let mut shard_worker = self.enum_workers_at_key(&key).await;
                result.append(&mut shard_worker);
            }
        }
        result
    }

    async fn remove(&self, owned_agent_id: &OwnedAgentId) {
        record_worker_call("remove");

        if let Some(agent_mode) = self.get_agent_mode(owned_agent_id).await {
            self.oplog_service.delete(owned_agent_id, agent_mode).await;
        }
        self.remove_cached_status(owned_agent_id).await;

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assigment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);

        self
            .key_value_storage
            .with_entity("worker", "remove", "agent_id")
            .remove_from_set(KeyValueStorageNamespace::RunningWorkers, &Self::running_in_shard_key(&shard_id), owned_agent_id)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove worker from the set of running worker ids per shard in KV storage: {err}"
                )
            });
    }

    async fn remove_cached_status(&self, owned_agent_id: &OwnedAgentId) {
        record_worker_call("remove_cached_status");

        self.key_value_storage
            .with("worker", "remove")
            .del_many(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                vec![
                    Self::status_key(&owned_agent_id.agent_id),
                    Self::agent_mode_key(&owned_agent_id.agent_id),
                ],
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove worker status in the KV storage: {err}")
            });
    }

    async fn get_agent_mode(&self, owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
        record_worker_call("get_agent_mode");

        // Fast path: dedicated `agent_mode` key (only populated for durable workers).
        if let Some(agent_mode) = self.read_cached_agent_mode(owned_agent_id).await {
            return Some(agent_mode);
        }

        // Cache miss (e.g. ephemeral worker, or durable worker whose dedicated key has not been
        // populated yet): probe both oplog namespaces. Constant-time existence checks.
        if self
            .oplog_service
            .exists(owned_agent_id, AgentMode::Durable)
            .await
        {
            // Populate the dedicated key so subsequent lookups skip the oplog probe. Only durable
            // workers are cached (mirrors `update_cached_status`): an ephemeral worker's oplog is
            // transient, so caching its mode could outlive the worker and return a stale `Some`.
            self.write_cached_agent_mode(owned_agent_id, AgentMode::Durable)
                .await;
            Some(AgentMode::Durable)
        } else if self
            .oplog_service
            .exists(owned_agent_id, AgentMode::Ephemeral)
            .await
        {
            Some(AgentMode::Ephemeral)
        } else {
            None
        }
    }

    async fn update_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        status_value: &AgentStatusRecord,
    ) {
        record_worker_call("update_status");

        if status_value.agent_mode != AgentMode::Ephemeral {
            debug!("Updating cached worker status for {owned_agent_id} to {status_value:?}");

            self.key_value_storage
                .with_entity("worker", "update_status", "worker_status")
                .set(
                    KeyValueStorageNamespace::Worker {
                        agent_id: owned_agent_id.agent_id(),
                    },
                    &Self::status_key(&owned_agent_id.agent_id),
                    status_value,
                )
                .await
                .unwrap_or_else(|err| panic!("failed to set worker status in KV storage: {err}"));

            let shard_assignment = self
                .shard_service
                .current_assignment()
                .expect("sharding assignment is not ready");

            let shard_id =
                ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);

            if Self::should_track_for_assignment_recovery(status_value) {
                debug!("Adding worker to the set of running workers in shard {shard_id}");

                self
                    .key_value_storage
                    .with_entity("worker", "add", "agent_id")
                    .add_to_set(KeyValueStorageNamespace::RunningWorkers, &Self::running_in_shard_key(&shard_id), owned_agent_id)
                    .await
                    .unwrap_or_else(|err| {
                        panic!(
                            "failed to add worker to the set of running workers per shard ids on KV storage: {err}"
                        )
                    });
            } else {
                debug!("Removing worker from the set of running workers in shard {shard_id}");

                self
                    .key_value_storage
                    .with_entity("worker", "remove", "agent_id")
                    .remove_from_set(KeyValueStorageNamespace::RunningWorkers, &Self::running_in_shard_key(&shard_id), owned_agent_id)
                    .await
                    .unwrap_or_else(|err| {
                        panic!(
                            "failed to remove worker from the set of running worker ids per shard on KV storage: {err}"
                        )
                    });
            }
        }
    }
}

impl HasOplogService for DefaultWorkerService {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        self.oplog_service.clone()
    }
}

impl HasConfig for DefaultWorkerService {
    fn config(&self) -> Arc<GolemConfig> {
        self.config.clone()
    }
}

impl HasComponentService for DefaultWorkerService {
    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentInvocation;
    use golem_common::model::Timestamp;
    use golem_common::model::TimestampedAgentInvocation;
    use golem_common::model::oplog::{TimestampedUpdateDescription, UpdateDescription};
    use std::collections::VecDeque;
    use test_r::test;

    #[test]
    fn tracks_workers_with_pending_invocations_for_assignment_recovery() {
        let mut status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };
        status.pending_invocations.push(TimestampedAgentInvocation {
            timestamp: Timestamp::now_utc(),
            invocation: AgentInvocation::ManualUpdate {
                target_revision: golem_common::model::component::ComponentRevision::INITIAL,
            },
        });

        assert!(DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }

    #[test]
    fn tracks_workers_with_pending_updates_for_assignment_recovery() {
        let status = AgentStatusRecord {
            status: AgentStatus::Idle,
            pending_updates: VecDeque::from([TimestampedUpdateDescription {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::INITIAL,
                description: UpdateDescription::Automatic {
                    target_revision: golem_common::model::component::ComponentRevision::INITIAL,
                },
            }]),
            ..AgentStatusRecord::default()
        };

        assert!(DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }

    #[test]
    fn does_not_track_idle_workers_without_pending_work() {
        let status = AgentStatusRecord {
            status: AgentStatus::Idle,
            ..AgentStatusRecord::default()
        };

        assert!(!DefaultWorkerService::should_track_for_assignment_recovery(
            &status
        ));
    }
}
