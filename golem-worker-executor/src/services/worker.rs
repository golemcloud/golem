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

use super::golem_config::GolemConfig;
use super::{HasConfig, HasOplogService};
use crate::metrics::workers::record_worker_call;
use crate::services::oplog::OplogService;
use crate::services::shard::ShardService;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::status::calculate_last_known_status_for_existing_worker;
use async_trait::async_trait;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{
    AgentId, AgentMetadata, AgentStatus, AgentStatusRecord, OwnedAgentId, ShardId,
};
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

    async fn update_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        status_value: &AgentStatusRecord,
        agent_mode: AgentMode,
    );
}

#[derive(Clone)]
pub struct DefaultWorkerService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    shard_service: Arc<dyn ShardService>,
    oplog_service: Arc<dyn OplogService>,
    config: Arc<GolemConfig>,
}

impl DefaultWorkerService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        oplog_service: Arc<dyn OplogService>,
        config: Arc<GolemConfig>,
    ) -> Self {
        Self {
            key_value_storage,
            shard_service,
            oplog_service,
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

    fn running_in_shard_key(shard_id: &ShardId) -> String {
        format!("worker:running_in_shard:{shard_id}")
    }
}

#[async_trait]
impl WorkerService for DefaultWorkerService {
    async fn get(&self, owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult> {
        record_worker_call("get");

        let initial_oplog_entry = self
            .oplog_service
            .read(owned_agent_id, OplogIndex::INITIAL, 1)
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
                    component_revision,
                    env,
                    environment_id,
                    created_by,
                    timestamp,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins,
                    config_vars,
                    original_phantom_id,
                },
            )) => {
                let initial_worker_metadata = AgentMetadata {
                    agent_id,
                    env,
                    config_vars,
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
                        ..AgentStatusRecord::default()
                    },
                    original_phantom_id,
                };

                let status_value: Option<Result<AgentStatusRecord, String>> = self
                    .key_value_storage
                    .with_entity("worker", "get", "worker_status")
                    .get_attempt_deserialize(
                        KeyValueStorageNamespace::Worker { agent_id: owned_agent_id.agent_id.clone() },
                        &Self::status_key(&owned_agent_id.agent_id),
                    )
                    .await
                    .unwrap_or_else(|err| {
                        panic!("failed to get worker status for {owned_agent_id} from KV storage: {err}")
                    });

                let last_known_status = match status_value {
                    Some(Ok(status)) => Some(status),
                    // We had a status, but it was written in a previous format and is no longer valid -> recompute
                    Some(Err(_)) => {
                        let last_known_status = calculate_last_known_status_for_existing_worker(
                            self,
                            owned_agent_id,
                            None,
                        )
                        .await;

                        self.update_cached_status(
                            owned_agent_id,
                            &last_known_status,
                            AgentMode::Durable,
                        )
                        .await;

                        Some(last_known_status)
                    }
                    None => None,
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

        self.oplog_service.delete(owned_agent_id).await;
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
            .del(
                KeyValueStorageNamespace::Worker {
                    agent_id: owned_agent_id.agent_id(),
                },
                &Self::status_key(&owned_agent_id.agent_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove worker status in the KV storage: {err}")
            });
    }

    async fn update_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        status_value: &AgentStatusRecord,
        agent_mode: AgentMode,
    ) {
        record_worker_call("update_status");

        if agent_mode != AgentMode::Ephemeral {
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

            if status_value.status == AgentStatus::Running {
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
