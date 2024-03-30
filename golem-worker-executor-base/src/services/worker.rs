// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use golem_common::config::RetryConfig;
use golem_common::metrics::redis::record_redis_serialized_size;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{ShardId, WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord};
use golem_common::redis::RedisPool;
use tracing::debug;

use crate::error::GolemError;
use crate::metrics::workers::record_worker_call;
use crate::services::golem_config::WorkersServiceConfig;
use crate::services::oplog::OplogService;
use crate::services::shard::ShardService;

/// Service for persisting the current set of Golem workers represented by their metadata
#[async_trait]
pub trait WorkerService {
    async fn add(&self, worker_metadata: &WorkerMetadata) -> Result<(), GolemError>;

    async fn get(&self, worker_id: &WorkerId) -> Option<WorkerMetadata>;

    async fn get_running_workers_in_shards(&self) -> Vec<WorkerMetadata>;

    async fn remove(&self, worker_id: &WorkerId);

    async fn enumerate(&self) -> Vec<WorkerMetadata>;

    async fn update_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        deleted_regions: DeletedRegions,
        overridden_retry_config: Option<RetryConfig>,
        oplog_idx: u64,
    );
}

#[derive(Clone)]
pub struct WorkerServiceRedis {
    redis: RedisPool,
    shard_service: Arc<dyn ShardService + Send + Sync>,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
}

impl WorkerServiceRedis {
    pub fn new(
        redis: RedisPool,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
    ) -> Self {
        Self {
            redis,
            shard_service,
            oplog_service,
        }
    }

    async fn enum_workers_at_key(&self, key: &str) -> Vec<WorkerMetadata> {
        record_worker_call("enum");

        let value: Vec<Bytes> = self
            .redis
            .with("instance", "enum")
            .smembers(key)
            .await
            .unwrap_or_else(|err| panic!("failed to get worker ids from Redis: {err}"));

        let mut workers = Vec::new();

        for worker in value {
            let worker_id: WorkerId = self
                .redis
                .deserialize(&worker)
                .unwrap_or_else(|err| panic!("failed to deserialize worker id: {worker:?}: {err}"));

            let metadata = self.get(&worker_id).await.unwrap_or_else(|| {
                panic!("failed to get worker metadata for {worker_id} from Redis")
            });
            workers.push(metadata);
        }

        workers
    }
}

#[async_trait]
impl WorkerService for WorkerServiceRedis {
    async fn add(&self, worker_metadata: &WorkerMetadata) -> Result<(), GolemError> {
        record_worker_call("add");

        let worker_id = &worker_metadata.worker_id.worker_id;

        let initial_oplog_entry = OplogEntry::create(
            worker_metadata.worker_id.clone(),
            worker_metadata.args.clone(),
            worker_metadata.env.clone(),
            worker_metadata.account_id.clone(),
        );
        self.oplog_service
            .create(worker_id, initial_oplog_entry)
            .await;

        let status_key = get_worker_status_redis_key(worker_id);
        let status_value = self
            .redis
            .serialize(&worker_metadata.last_known_status)
            .unwrap_or_else(|err| {
                panic!(
                    "failed to serialize worker status {:?}: {err}",
                    worker_metadata.last_known_status
                )
            });

        record_redis_serialized_size("instance", "status", status_value.len());

        self.redis
            .with("instance", "add")
            .set(status_key.clone(), status_value, None, None, false)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to set worker status for {status_key} in Redis: {err}")
            });

        let serialized_worker_id = self
            .redis
            .serialize(&worker_id)
            .expect("failed to serialize worker id");

        if worker_metadata.last_known_status.status == WorkerStatus::Running {
            let shard_assignment = self.shard_service.current_assignment();
            let shard_id = ShardId::from_worker_id(worker_id, shard_assignment.number_of_shards);
            let running_workers_in_shard_key = get_running_worker_per_shard_key(&shard_id);

            debug!("Adding worker id {worker_id} to the list of running workers for shard {shard_id} on Redis");

            let _: u32 = self
                .redis
                .with("instance", "add")
                .sadd(running_workers_in_shard_key, serialized_worker_id.clone())
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to add worker id {worker_id} to the set of running workers per shard ids on Redis: {err}"
                    )
                });
        }

        let key = "instance:instance";
        debug!("Adding worker id {worker_id} to the set of workers on Redis");

        let _: u32 = self
            .redis
            .with("instance", "add")
            .sadd(key, serialized_worker_id)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to add worker id {worker_id} to the set of workers ids on Redis: {err}"
                )
            });

        Ok(())
    }

    async fn get(&self, worker_id: &WorkerId) -> Option<WorkerMetadata> {
        record_worker_call("get");

        let initial_oplog_entry = self
            .oplog_service
            .read(worker_id, 0, 1)
            .await
            .into_iter()
            .next();

        let status_key = get_worker_status_redis_key(worker_id);

        match initial_oplog_entry {
            None => None,
            Some(OplogEntry::Create {
                worker_id,
                args,
                env,
                account_id,
                timestamp,
            }) => {
                let mut details = WorkerMetadata {
                    worker_id,
                    args,
                    env,
                    account_id,
                    created_at: timestamp,
                    last_known_status: WorkerStatusRecord::default(),
                };

                let status_value: Option<Bytes> = self
                    .redis
                    .with("instance", "get")
                    .get(status_key.clone())
                    .await
                    .unwrap_or_else(|err| {
                        panic!("failed to get worker status for {status_key} on Redis: {err}")
                    });

                if let Some(status_value) = status_value {
                    let status = self.redis.deserialize(&status_value).unwrap_or_else(|err| {
                        panic!(
                            "failed to deserialize worker status for {status_key} on Redis: {err}"
                        )
                    });

                    details.last_known_status = status;
                }

                Some(details)
            }
            Some(entry) => {
                panic!("Unexpected initial oplog entry for worker {worker_id}: {entry:?}")
            }
        }
    }

    async fn get_running_workers_in_shards(&self) -> Vec<WorkerMetadata> {
        let shard_assignment = self.shard_service.current_assignment();
        let mut result: Vec<WorkerMetadata> = vec![];
        for shard_id in shard_assignment.shard_ids {
            let key = get_running_worker_per_shard_key(&shard_id);
            let mut shard_worker = self.enum_workers_at_key(&key).await;
            result.append(&mut shard_worker);
        }
        result
    }

    async fn remove(&self, worker_id: &WorkerId) {
        record_worker_call("remove");

        let key = "instance:instance";
        let serialized_worker_id = self
            .redis
            .serialize(&worker_id)
            .expect("failed to serialize worker id");

        let _: u32 = self
            .redis
            .with("instance", "remove")
            .srem(key, serialized_worker_id.clone())
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove worker id {worker_id} from the set of worker ids on Redis: {err}")
            });

        self.oplog_service.delete(worker_id).await;

        let status_key = get_worker_status_redis_key(worker_id);
        let _: u32 = self
            .redis
            .with("instance", "remove")
            .del(status_key.clone())
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove worker status for {status_key} on Redis: {err}")
            });

        let shard_assignment = self.shard_service.current_assignment();
        let shard_id = ShardId::from_worker_id(worker_id, shard_assignment.number_of_shards);
        let running_workers_in_shard_key = get_running_worker_per_shard_key(&shard_id);

        let _: u32 = self
            .redis
            .with("instance", "remove")
            .srem(running_workers_in_shard_key, serialized_worker_id)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove worker id {worker_id} from the set of running worker ids per shard on Redis: {err}"
                )
            });
    }

    async fn enumerate(&self) -> Vec<WorkerMetadata> {
        let key = "instance:instance";
        self.enum_workers_at_key(key).await
    }

    async fn update_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        deleted_regions: DeletedRegions,
        overridden_retry_config: Option<RetryConfig>,
        oplog_idx: u64,
    ) {
        record_worker_call("update_status");

        let status_key = get_worker_status_redis_key(worker_id);
        let status_value = WorkerStatusRecord {
            status: status.clone(),
            deleted_regions,
            overridden_retry_config,
            oplog_idx,
        };
        let serialized_status_value = self.redis.serialize(&status_value).unwrap_or_else(|err| {
            panic!(
                "failed to serialize worker status to {:?} in Redis: {err}",
                status_value
            )
        });

        debug!("updating worker status for {worker_id} to {status_value:?}");
        self.redis
            .with("instance", "update_status")
            .set(
                status_key.clone(),
                serialized_status_value,
                None,
                None,
                false,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to set worker status for {status_key} in Redis: {err}")
            });

        let shard_assignment = self.shard_service.current_assignment();
        let shard_id = ShardId::from_worker_id(worker_id, shard_assignment.number_of_shards);
        let running_workers_in_shard_key = get_running_worker_per_shard_key(&shard_id);

        let serialized_worker_id = self
            .redis
            .serialize(&worker_id)
            .expect("failed to serialize worker id");

        if status == WorkerStatus::Running {
            debug!("adding worker {worker_id} to the set of running workers in shard {shard_id}");

            let _: u32 = self
                .redis
                .with("instance", "add")
                .sadd(running_workers_in_shard_key, serialized_worker_id.clone())
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to add worker id {worker_id} from the set of running workers per shard ids on Redis: {err}"
                    )
                });
        } else {
            debug!(
                "removing instance {worker_id} from the set of running workers in shard {shard_id}"
            );

            let _: u32 = self
                .redis
                .with("instance", "remove")
                .srem(running_workers_in_shard_key, serialized_worker_id)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to remove worker id {worker_id} from the set of running worker ids per shard on Redis: {err}"
                    )
                });
        }
    }
}

fn get_worker_status_redis_key(worker_id: &WorkerId) -> String {
    format!("instance:status:{}", worker_id.to_redis_key())
}

fn get_running_worker_per_shard_key(shard_id: &ShardId) -> String {
    format!("instance:running_in_shard:{shard_id}")
}

pub struct WorkerServiceInMemory {
    workers: Arc<DashMap<WorkerId, WorkerMetadata>>,
}

impl Default for WorkerServiceInMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerServiceInMemory {
    pub fn new() -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
        }
    }
}

#[async_trait]
impl WorkerService for WorkerServiceInMemory {
    async fn add(&self, worker_metadata: &WorkerMetadata) -> Result<(), GolemError> {
        self.workers.insert(
            worker_metadata.worker_id.worker_id.clone(),
            worker_metadata.clone(),
        );
        Ok(())
    }

    async fn get(&self, worker_id: &WorkerId) -> Option<WorkerMetadata> {
        self.workers
            .get(worker_id)
            .map(|worker| worker.value().clone())
    }

    async fn get_running_workers_in_shards(&self) -> Vec<WorkerMetadata> {
        self.workers
            .iter()
            .filter(|r| r.last_known_status.status == WorkerStatus::Running)
            .map(|i| i.clone())
            .collect()
    }

    async fn remove(&self, worker_id: &WorkerId) {
        self.workers.remove(worker_id);
    }

    async fn enumerate(&self) -> Vec<WorkerMetadata> {
        self.workers.iter().map(|i| i.clone()).collect()
    }

    async fn update_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        deleted_regions: DeletedRegions,
        overridden_retry_config: Option<RetryConfig>,
        oplog_idx: u64,
    ) {
        self.workers.entry(worker_id.clone()).and_modify(|worker| {
            worker.last_known_status = WorkerStatusRecord {
                status,
                deleted_regions,
                overridden_retry_config,
                oplog_idx,
            }
        });
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct WorkerServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for WorkerServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl WorkerServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl WorkerService for WorkerServiceMock {
    async fn add(&self, _worker_metadata: &WorkerMetadata) -> Result<(), GolemError> {
        unimplemented!()
    }

    async fn get(&self, _worker_id: &WorkerId) -> Option<WorkerMetadata> {
        unimplemented!()
    }

    async fn get_running_workers_in_shards(&self) -> Vec<WorkerMetadata> {
        unimplemented!()
    }

    async fn remove(&self, _worker_id: &WorkerId) {
        unimplemented!()
    }

    async fn enumerate(&self) -> Vec<WorkerMetadata> {
        unimplemented!()
    }

    async fn update_status(
        &self,
        _worker_id: &WorkerId,
        _status: WorkerStatus,
        _deleted_regions: DeletedRegions,
        _overridden_retry_config: Option<RetryConfig>,
        _oplog_idx: u64,
    ) {
        unimplemented!()
    }
}
