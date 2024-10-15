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

use std::collections::HashSet;
use std::ops::Add;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{error, info, span, warn, Instrument, Level};

use crate::metrics::oplog::record_scheduled_archive;
use crate::metrics::promises::record_scheduled_promise_completed;
use crate::services::oplog::{MultiLayerOplog, OplogService};
use crate::services::promise::PromiseService;
use crate::services::shard::ShardService;
use crate::services::worker::WorkerService;
use crate::services::worker_activator::WorkerActivator;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_common::model::{ComponentType, ScheduleId, ScheduledAction};

#[async_trait]
pub trait SchedulerService {
    async fn schedule(&self, time: DateTime<Utc>, action: ScheduledAction) -> ScheduleId;

    async fn cancel(&self, id: ScheduleId);
}

#[derive(Clone)]
pub struct SchedulerServiceDefault {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shard_service: Arc<dyn ShardService + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
}

impl SchedulerServiceDefault {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        process_interval: Duration,
    ) -> Arc<Self> {
        let svc = Self {
            key_value_storage,
            background_handle: Arc::new(Mutex::new(None)),
            shard_service,
            promise_service,
            oplog_service,
            worker_service,
            worker_activator,
        };
        let svc = Arc::new(svc);
        let background_handle = {
            let svc = svc.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(process_interval).await;
                    if svc.shard_service.is_ready() {
                        let r = svc.process(Utc::now()).await;
                        if let Err(err) = r {
                            error!(err, "Error in scheduler background task");
                        }
                    } else {
                        warn!("Skipping schedule, shard service is not ready")
                    }
                }
            })
        };
        *svc.background_handle.lock().unwrap() = Some(background_handle);

        svc
    }

    async fn process(&self, now: DateTime<Utc>) -> Result<(), String> {
        let (hours_since_epoch, remainder) = Self::split_time(now);
        let previous_hours_since_epoch = hours_since_epoch - 1;

        let previous_hour_key = Self::schedule_key_from_timestamp(previous_hours_since_epoch);
        let current_hour_key = Self::schedule_key_from_timestamp(hours_since_epoch);

        let all_from_prev_hour: Vec<(f64, ScheduledAction)> = self
            .key_value_storage
            .with_entity("scheduler", "process", "scheduled_action")
            .get_sorted_set(KeyValueStorageNamespace::Schedule, &previous_hour_key)
            .await?;

        let mut all: Vec<(&str, ScheduledAction)> = all_from_prev_hour
            .into_iter()
            .map(|(_score, action)| (previous_hour_key.as_str(), action))
            .collect();

        let all_from_this_hour: Vec<(f64, ScheduledAction)> = self
            .key_value_storage
            .with_entity("scheduler", "process", "scheduled_action")
            .query_sorted_set(
                KeyValueStorageNamespace::Schedule,
                &current_hour_key,
                0.0,
                remainder,
            )
            .await?;

        all.extend(
            all_from_this_hour
                .into_iter()
                .map(|(_score, action)| (current_hour_key.as_str(), action)),
        );

        let matching: Vec<(&str, ScheduledAction)> = all
            .into_iter()
            .filter(|(_, action)| {
                self.shard_service
                    .check_worker(&action.owned_worker_id().worker_id)
                    .is_ok()
            })
            .collect::<Vec<_>>();

        let mut owned_worker_ids = HashSet::new();
        for (key, action) in matching {
            owned_worker_ids.insert(action.owned_worker_id().clone());
            self.key_value_storage
                .with_entity("scheduler", "process", "scheduled_action")
                .remove_from_sorted_set(KeyValueStorageNamespace::Schedule, key, &action)
                .await?;

            match action {
                ScheduledAction::CompletePromise { promise_id, .. } => {
                    self.promise_service
                        .complete(promise_id, vec![])
                        .await
                        .map_err(|golem_err| format!("{golem_err}"))?;

                    record_scheduled_promise_completed();
                }
                ScheduledAction::ArchiveOplog {
                    owned_worker_id,
                    last_oplog_index,
                    next_after,
                } => {
                    if self.oplog_service.exists(&owned_worker_id).await {
                        let current_last_index =
                            self.oplog_service.get_last_index(&owned_worker_id).await;
                        if current_last_index == last_oplog_index {
                            // We never schedule an archive operation for ephemeral workers, because they immediately write their oplog to the arcchive layer
                            // So we can assume the component type is Durable here without calculating it from the latest component and worker metadata.
                            let oplog = self
                                .oplog_service
                                .open(&owned_worker_id, last_oplog_index, ComponentType::Durable)
                                .await;

                            let start = Instant::now();
                            if let Some(more) = MultiLayerOplog::try_archive(&oplog).await {
                                record_scheduled_archive(start.elapsed(), more);
                                if more {
                                    self.schedule(
                                        now.add(next_after),
                                        ScheduledAction::ArchiveOplog {
                                            owned_worker_id,
                                            last_oplog_index,
                                            next_after,
                                        },
                                    )
                                    .await;
                                } else {
                                    info!(
                                        worker_id = owned_worker_id.to_string(),
                                        "Deleting cached status of fully archived worker"
                                    );
                                    // The oplog is fully archived, so we can also delete the cached worker status
                                    self.worker_service
                                        .remove_cached_status(&owned_worker_id)
                                        .await;
                                }
                            }
                        }

                        // TODO: metrics
                    }
                }
            }
        }

        for owned_worker_id in owned_worker_ids {
            let span = span!(
                Level::INFO,
                "scheduler",
                worker_id = owned_worker_id.worker_id.to_string()
            );
            self.worker_activator
                .activate_worker(&owned_worker_id)
                .instrument(span)
                .await;
        }

        Ok(())
    }

    const HOUR_IN_MILLIS: i64 = 1000 * 60 * 60;

    fn split_time<Tz: TimeZone>(time: DateTime<Tz>) -> (i64, f64) {
        let millis = time.timestamp_millis();
        let hours_since_epoch = millis / Self::HOUR_IN_MILLIS;
        let remainder = (millis % Self::HOUR_IN_MILLIS) as f64;
        (hours_since_epoch, remainder)
    }

    fn schedule_key(id: &ScheduleId) -> String {
        Self::schedule_key_from_timestamp(id.timestamp)
    }

    fn schedule_key_from_timestamp(timestamp: i64) -> String {
        format!("worker:schedule:{}", timestamp)
    }
}

impl Drop for SchedulerServiceDefault {
    fn drop(&mut self) {
        if let Some(handle) = self.background_handle.lock().unwrap().take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl SchedulerService for SchedulerServiceDefault {
    async fn schedule(&self, time: DateTime<Utc>, action: ScheduledAction) -> ScheduleId {
        let (hours_since_epoch, remainder) = Self::split_time(time);

        let id = ScheduleId {
            timestamp: hours_since_epoch,
            action: action.clone(),
        };

        self.key_value_storage
            .with_entity("scheduler", "schedule", "scheduled_action")
            .add_to_sorted_set(
                KeyValueStorageNamespace::Schedule,
                &Self::schedule_key(&id),
                remainder,
                &action,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to add schedule for action {action} in KV storage: {err}")
            });

        id
    }

    async fn cancel(&self, id: ScheduleId) {
        self.key_value_storage
            .with_entity("scheduler", "cancel", "scheduled_action")
            .remove_from_sorted_set(
                KeyValueStorageNamespace::Schedule,
                &Self::schedule_key(&id),
                &id.action,
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove schedule for action {} from KV storage: {err}",
                    id.action
                )
            });
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use std::collections::{HashMap, HashSet};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use bincode::Encode;

    use chrono::DateTime;

    use uuid::Uuid;

    use crate::services::oplog::{OplogService, PrimaryOplogService};
    use crate::services::promise::PromiseServiceMock;
    use crate::services::scheduler::{SchedulerService, SchedulerServiceDefault};
    use crate::services::shard::{ShardService, ShardServiceDefault};
    use crate::services::worker::{DefaultWorkerService, WorkerService};
    use crate::services::worker_activator::{WorkerActivator, WorkerActivatorMock};
    use crate::storage::blob::memory::InMemoryBlobStorage;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{
        AccountId, ComponentId, PromiseId, ScheduledAction, ShardId, WorkerId,
    };

    fn serialized_bytes<T: Encode>(entry: &T) -> Vec<u8> {
        golem_common::serialization::serialize(entry)
            .expect("failed to serialize entry")
            .to_vec()
    }

    fn create_shard_service_mock() -> Arc<dyn ShardService + Send + Sync> {
        let result = Arc::new(ShardServiceDefault::new());
        result.register(1, &HashSet::from_iter(vec![ShardId::new(0)]));
        result
    }

    fn create_promise_service_mock() -> Arc<PromiseServiceMock> {
        Arc::new(PromiseServiceMock::new())
    }

    fn create_worker_activator_mock() -> Arc<dyn WorkerActivator + Send + Sync> {
        Arc::new(WorkerActivatorMock::new())
    }

    async fn create_oplog_service_mock() -> Arc<dyn OplogService + Send + Sync> {
        Arc::new(
            PrimaryOplogService::new(
                Arc::new(InMemoryIndexedStorage::new()),
                Arc::new(InMemoryBlobStorage::new()),
                1,
                1024,
            )
            .await,
        )
    }

    fn create_worker_service_mock(
        kvs: Arc<InMemoryKeyValueStorage>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
    ) -> Arc<dyn WorkerService + Send + Sync> {
        Arc::new(DefaultWorkerService::new(kvs, shard_service, oplog_service))
    }

    #[test]
    pub async fn promises_added_to_expected_buckets() {
        let uuid = Uuid::new_v4();
        let c1: ComponentId = ComponentId(uuid);
        let i1: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let account_id = AccountId {
            value: "test-account".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(101),
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(123),
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: OplogIndex::from_u64(1000),
        };

        let kvs = Arc::new(InMemoryKeyValueStorage::new());

        let shard_service = create_shard_service_mock();
        let promise_service = create_promise_service_mock();
        let worker_activator = create_worker_activator_mock();
        let oplog_service = create_oplog_service_mock().await;
        let worker_service =
            create_worker_service_mock(kvs.clone(), shard_service.clone(), oplog_service.clone());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service,
            worker_activator,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // not testing process() here
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p1.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p2.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p3.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            result,
            HashMap::from_iter(vec![
                (
                    "Schedule/worker:schedule:469329".to_string(),
                    vec![(
                        3540000.0,
                        serialized_bytes(&ScheduledAction::CompletePromise {
                            promise_id: p2,
                            account_id: account_id.clone()
                        })
                    )]
                ),
                (
                    "Schedule/worker:schedule:469330".to_string(),
                    vec![
                        (
                            300000.0,
                            serialized_bytes(&ScheduledAction::CompletePromise {
                                promise_id: p1,
                                account_id: account_id.clone()
                            })
                        ),
                        (
                            301000.0,
                            serialized_bytes(&ScheduledAction::CompletePromise {
                                promise_id: p3,
                                account_id: account_id.clone()
                            })
                        )
                    ]
                )
            ])
        );
    }

    #[test]
    pub async fn cancel_removes_entry() {
        let c1: ComponentId = ComponentId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let account_id = AccountId {
            value: "test-account".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(101),
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(123),
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: OplogIndex::from_u64(1000),
        };

        let kvs = Arc::new(InMemoryKeyValueStorage::new());

        let shard_service = create_shard_service_mock();
        let promise_service = create_promise_service_mock();
        let worker_activator = create_worker_activator_mock();
        let oplog_service = create_oplog_service_mock().await;
        let worker_service =
            create_worker_service_mock(kvs.clone(), shard_service.clone(), oplog_service.clone());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service,
            worker_activator,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // not testing process() here
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p1.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p2.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p3.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;

        svc.cancel(s2).await;
        svc.cancel(s3).await;

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            result,
            HashMap::from([
                ("Schedule/worker:schedule:469329".to_string(), vec![]),
                (
                    "Schedule/worker:schedule:469330".to_string(),
                    vec![(
                        300000.0,
                        serialized_bytes(&ScheduledAction::CompletePromise {
                            promise_id: p1,
                            account_id: account_id.clone()
                        })
                    )]
                )
            ])
        );
    }

    #[test]
    pub async fn process_current_hours_past_schedules() {
        let c1: ComponentId = ComponentId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let account_id = AccountId {
            value: "test-account".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(101),
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(123),
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: OplogIndex::from_u64(1000),
        };

        let kvs = Arc::new(InMemoryKeyValueStorage::new());

        let shard_service = create_shard_service_mock();
        let promise_service = create_promise_service_mock();
        let worker_activator = create_worker_activator_mock();
        let oplog_service = create_oplog_service_mock().await;
        let worker_service =
            create_worker_service_mock(kvs.clone(), shard_service.clone(), oplog_service.clone());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p1.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p2.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p3.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([(
                "Schedule/worker:schedule:469330".to_string(),
                vec![(
                    3540000.0,
                    serialized_bytes(&ScheduledAction::CompletePromise {
                        promise_id: p2.clone(),
                        account_id: account_id.clone()
                    })
                )]
            )])
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(!completed_promises.contains(&p2));
    }

    #[test]
    pub async fn process_past_and_current_hours_past_schedules() {
        let c1: ComponentId = ComponentId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let account_id = AccountId {
            value: "test-account".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(101),
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(123),
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: OplogIndex::from_u64(1000),
        };

        let kvs = Arc::new(InMemoryKeyValueStorage::new());

        let shard_service = create_shard_service_mock();
        let promise_service = create_promise_service_mock();
        let worker_activator = create_worker_activator_mock();
        let oplog_service = create_oplog_service_mock().await;
        let worker_service =
            create_worker_service_mock(kvs.clone(), shard_service.clone(), oplog_service.clone());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p1.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p2.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p3.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([
                ("Schedule/worker:schedule:469329".to_string(), vec![]),
                ("Schedule/worker:schedule:469330".to_string(), vec![])
            ])
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(completed_promises.contains(&p2));
    }

    #[test]
    pub async fn process_past_and_current_hours_past_schedules_2() {
        let c1: ComponentId = ComponentId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let account_id = AccountId {
            value: "test-account".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(101),
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(123),
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: OplogIndex::from_u64(1000),
        };
        let p4: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(111),
        };

        let kvs = Arc::new(InMemoryKeyValueStorage::new());

        let shard_service = create_shard_service_mock();
        let promise_service = create_promise_service_mock();
        let worker_activator = create_worker_activator_mock();
        let oplog_service = create_oplog_service_mock().await;
        let worker_service =
            create_worker_service_mock(kvs.clone(), shard_service.clone(), oplog_service.clone());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p1.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p2.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p3.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s4 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:47:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p4.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([
                ("Schedule/worker:schedule:469329".to_string(), vec![]),
                ("Schedule/worker:schedule:469330".to_string(), vec![])
            ])
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(completed_promises.contains(&p2));
        assert!(completed_promises.contains(&p4));
    }

    #[test]
    pub async fn process_past_and_current_hours_past_schedules_3() {
        let c1: ComponentId = ComponentId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            component_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let account_id = AccountId {
            value: "test-account".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(101),
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: OplogIndex::from_u64(123),
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: OplogIndex::from_u64(1000),
        };

        let kvs = Arc::new(InMemoryKeyValueStorage::new());

        let shard_service = create_shard_service_mock();
        let promise_service = create_promise_service_mock();
        let worker_activator = create_worker_activator_mock();
        let oplog_service = create_oplog_service_mock().await;
        let worker_service =
            create_worker_service_mock(kvs.clone(), shard_service.clone(), oplog_service.clone());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p1.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p2.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:47:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    promise_id: p3.clone(),
                    account_id: account_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([
                ("Schedule/worker:schedule:469329".to_string(), vec![]),
                ("Schedule/worker:schedule:469330".to_string(), vec![])
            ])
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(completed_promises.contains(&p2));
    }
}
