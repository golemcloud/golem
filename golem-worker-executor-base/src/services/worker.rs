// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::sync::{Arc, RwLock};

use crate::error::GolemError;
use crate::metrics::workers::record_worker_call;
use crate::model::ExecutionStatus;
use crate::services::oplog::OplogService;
use crate::services::shard::ShardService;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use async_trait::async_trait;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    ComponentType, OwnedWorkerId, ShardId, Timestamp, WorkerId, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord, WorkerStatusRecordExtensions,
};
use tracing::{debug, warn};

/// Service for persisting the current set of Golem workers represented by their metadata
#[async_trait]
pub trait WorkerService: Send + Sync {
    async fn add(
        &self,
        worker_metadata: &WorkerMetadata,
        component_type: ComponentType,
    ) -> Result<Arc<RwLock<ExecutionStatus>>, GolemError>;

    async fn get(&self, owned_worker_id: &OwnedWorkerId) -> Option<WorkerMetadata>;

    async fn get_running_workers_in_shards(&self) -> Vec<WorkerMetadata>;

    async fn remove(&self, owned_worker_id: &OwnedWorkerId);

    async fn remove_cached_status(&self, owned_worker_id: &OwnedWorkerId);

    async fn update_status(
        &self,
        owned_worker_id: &OwnedWorkerId,
        status_value: &WorkerStatusRecord,
        component_type: ComponentType,
    );
}

#[derive(Clone)]
pub struct DefaultWorkerService {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    shard_service: Arc<dyn ShardService>,
    oplog_service: Arc<dyn OplogService>,
}

impl DefaultWorkerService {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        oplog_service: Arc<dyn OplogService>,
    ) -> Self {
        Self {
            key_value_storage,
            shard_service,
            oplog_service,
        }
    }

    async fn enum_workers_at_key(&self, key: &str) -> Vec<WorkerMetadata> {
        record_worker_call("enum");

        let value: Vec<OwnedWorkerId> = self
            .key_value_storage
            .with_entity("worker", "enum", "worker_id")
            .members_of_set(KeyValueStorageNamespace::Worker, key)
            .await
            .unwrap_or_else(|err| panic!("failed to get worker ids from KV storage: {err}"));

        let mut workers = Vec::new();

        for owned_worker_id in value {
            let metadata = self
                .get(&owned_worker_id)
                .await
                .unwrap_or_else(|| panic!("failed to get worker metadata from KV storage"));
            workers.push(metadata);
        }

        workers
    }

    fn status_key(worker_id: &WorkerId) -> String {
        format!("worker:status:{}", worker_id.to_redis_key())
    }

    fn running_in_shard_key(shard_id: &ShardId) -> String {
        format!("worker:running_in_shard:{shard_id}")
    }
}

#[async_trait]
impl WorkerService for DefaultWorkerService {
    async fn add(
        &self,
        worker_metadata: &WorkerMetadata,
        component_type: ComponentType,
    ) -> Result<Arc<RwLock<ExecutionStatus>>, GolemError> {
        record_worker_call("add");

        let worker_id = &worker_metadata.worker_id;
        let owned_worker_id = OwnedWorkerId::new(&worker_metadata.account_id, worker_id);

        let initial_oplog_entry = OplogEntry::create(
            worker_metadata.worker_id.clone(),
            worker_metadata.last_known_status.component_version,
            worker_metadata.args.clone(),
            worker_metadata.env.clone(),
            worker_metadata.account_id.clone(),
            worker_metadata.parent.clone(),
            worker_metadata.last_known_status.component_size,
            worker_metadata.last_known_status.total_linear_memory_size,
            worker_metadata.last_known_status.active_plugins().clone(),
        );

        let execution_status = Arc::new(RwLock::new(ExecutionStatus::Suspended {
            last_known_status: worker_metadata.last_known_status.clone(),
            component_type,
            timestamp: Timestamp::now_utc(),
        }));

        self.oplog_service
            .create(
                &owned_worker_id,
                initial_oplog_entry,
                worker_metadata.clone(),
                execution_status.clone(),
            )
            .await;

        if component_type != ComponentType::Ephemeral {
            self.key_value_storage
                .with_entity("worker", "add", "worker_status")
                .set(
                    KeyValueStorageNamespace::Worker,
                    &Self::status_key(worker_id),
                    &worker_metadata.last_known_status,
                )
                .await
                .unwrap_or_else(|err| panic!("failed to set worker status in KV storage: {err}"));

            if worker_metadata.last_known_status.status == WorkerStatus::Running {
                let shard_assignment = self.shard_service.current_assignment()?;
                let shard_id =
                    ShardId::from_worker_id(worker_id, shard_assignment.number_of_shards);

                debug!(
                    "Adding worker to the list of running workers for shard {shard_id} in KV storage"
                );

                self
                    .key_value_storage
                    .with_entity("worker", "add", "worker_id")
                    .add_to_set(KeyValueStorageNamespace::Worker, &Self::running_in_shard_key(&shard_id), &owned_worker_id)
                    .await
                    .unwrap_or_else(|err| {
                        panic!(
                            "failed to add worker to the set of running workers per shard ids in KV storage: {err}"
                        )
                    });
            }
        }

        Ok(execution_status)
    }

    async fn get(&self, owned_worker_id: &OwnedWorkerId) -> Option<WorkerMetadata> {
        record_worker_call("get");

        let initial_oplog_entry = self
            .oplog_service
            .read(owned_worker_id, OplogIndex::INITIAL, 1)
            .await
            .into_iter()
            .next();

        match initial_oplog_entry {
            None => None,
            Some((
                _,
                OplogEntry::CreateV1 {
                    worker_id,
                    component_version,
                    args,
                    env,
                    account_id,
                    timestamp,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                },
            )) => {
                let mut details = WorkerMetadata {
                    worker_id,
                    args,
                    env,
                    account_id,
                    created_at: timestamp,
                    parent,
                    last_known_status: WorkerStatusRecord {
                        component_version,
                        component_size,
                        total_linear_memory_size: initial_total_linear_memory_size,
                        ..WorkerStatusRecord::default()
                    },
                };

                let status_value: Option<WorkerStatusRecord> = self
                    .key_value_storage
                    .with_entity("worker", "get", "worker_status")
                    .get(
                        KeyValueStorageNamespace::Worker,
                        &Self::status_key(&owned_worker_id.worker_id),
                    )
                    .await
                    .unwrap_or_else(|err| {
                        panic!("failed to get worker status for {owned_worker_id} from KV storage: {err}")
                    });

                if let Some(status) = status_value {
                    details.last_known_status = status;
                }

                Some(details)
            }
            Some((
                _,
                OplogEntry::Create {
                    worker_id,
                    component_version,
                    args,
                    env,
                    account_id,
                    timestamp,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins,
                },
            )) => {
                let mut details = WorkerMetadata {
                    worker_id,
                    args,
                    env,
                    account_id,
                    created_at: timestamp,
                    parent,
                    last_known_status: WorkerStatusRecord {
                        component_version,
                        component_size,
                        total_linear_memory_size: initial_total_linear_memory_size,
                        extensions: WorkerStatusRecordExtensions::Extension2 {
                            active_plugins: initial_active_plugins,
                            deleted_regions: DeletedRegions::new(),
                        },
                        ..WorkerStatusRecord::default()
                    },
                };

                let status_value: Option<WorkerStatusRecord> = self
                    .key_value_storage
                    .with_entity("worker", "get", "worker_status")
                    .get(
                        KeyValueStorageNamespace::Worker,
                        &Self::status_key(&owned_worker_id.worker_id),
                    )
                    .await
                    .unwrap_or_else(|err| {
                        panic!("failed to get worker status for {owned_worker_id} from KV storage: {err}")
                    });

                if let Some(status) = status_value {
                    details.last_known_status = status;
                }

                Some(details)
            }
            Some((_, entry)) => {
                // This should never happen, but there were some issues previously causing a corrupt oplog
                // leading to this state.
                //
                // There is no point in panicking and restarting the executor here, as the corrupt oplog
                // will most likely remain as it is.
                //
                // So to save the executor's state we return a "fake" failed worker metadata.

                warn!(
                    worker_id = owned_worker_id.to_string(),
                    oplog_entry = format!("{entry:?}"),
                    "Unexpected initial oplog entry found, returning fake failed worker metadata"
                );
                let last_oplog_idx = self.oplog_service.get_last_index(owned_worker_id).await;
                Some(WorkerMetadata {
                    worker_id: owned_worker_id.worker_id(),
                    args: vec![],
                    env: vec![],
                    account_id: owned_worker_id.account_id(),
                    created_at: Timestamp::now_utc(),
                    parent: None,
                    last_known_status: WorkerStatusRecord {
                        status: WorkerStatus::Failed,
                        oplog_idx: last_oplog_idx,
                        ..WorkerStatusRecord::default()
                    },
                })
            }
        }
    }

    async fn get_running_workers_in_shards(&self) -> Vec<WorkerMetadata> {
        let shard_assignment = self.shard_service.try_get_current_assignment();
        let mut result: Vec<WorkerMetadata> = vec![];
        if let Some(shard_assignment) = shard_assignment {
            for shard_id in shard_assignment.shard_ids {
                let key = Self::running_in_shard_key(&shard_id);
                let mut shard_worker = self.enum_workers_at_key(&key).await;
                result.append(&mut shard_worker);
            }
        }
        result
    }

    async fn remove(&self, owned_worker_id: &OwnedWorkerId) {
        record_worker_call("remove");

        self.oplog_service.delete(owned_worker_id).await;
        self.remove_cached_status(owned_worker_id).await;

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assigment is not ready");
        let shard_id = ShardId::from_worker_id(
            &owned_worker_id.worker_id,
            shard_assignment.number_of_shards,
        );

        self
            .key_value_storage
            .with_entity("worker", "remove", "worker_id")
            .remove_from_set(KeyValueStorageNamespace::Worker, &Self::running_in_shard_key(&shard_id), owned_worker_id)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove worker from the set of running worker ids per shard in KV storage: {err}"
                )
            });
    }

    async fn remove_cached_status(&self, owned_worker_id: &OwnedWorkerId) {
        record_worker_call("remove_cached_status");

        self.key_value_storage
            .with("worker", "remove")
            .del(
                KeyValueStorageNamespace::Worker,
                &Self::status_key(&owned_worker_id.worker_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove worker status in the KV storage: {err}")
            });
    }

    async fn update_status(
        &self,
        owned_worker_id: &OwnedWorkerId,
        status_value: &WorkerStatusRecord,
        component_type: ComponentType,
    ) {
        record_worker_call("update_status");

        if component_type != ComponentType::Ephemeral {
            self.key_value_storage
                .with_entity("worker", "update_status", "worker_status")
                .set(
                    KeyValueStorageNamespace::Worker,
                    &Self::status_key(&owned_worker_id.worker_id),
                    status_value,
                )
                .await
                .unwrap_or_else(|err| panic!("failed to set worker status in KV storage: {err}"));

            let shard_assignment = self
                .shard_service
                .current_assignment()
                .expect("sharding assignment is not ready");
            let shard_id = ShardId::from_worker_id(
                &owned_worker_id.worker_id,
                shard_assignment.number_of_shards,
            );

            if status_value.status == WorkerStatus::Running {
                debug!("Adding worker to the set of running workers in shard {shard_id}");

                self
                    .key_value_storage
                    .with_entity("worker", "add", "worker_id")
                    .add_to_set(KeyValueStorageNamespace::Worker, &Self::running_in_shard_key(&shard_id), owned_worker_id)
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
                    .with_entity("worker", "remove", "worker_id")
                    .remove_from_set(KeyValueStorageNamespace::Worker, &Self::running_in_shard_key(&shard_id), owned_worker_id)
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
