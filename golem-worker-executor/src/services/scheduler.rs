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

use crate::metrics::oplog::record_scheduled_archive;
use crate::metrics::promises::record_scheduled_promise_completed;
use crate::services::oplog::{MultiLayerOplog, Oplog, OplogService};
use crate::services::promise::PromiseService;
use crate::services::shard::ShardService;
use crate::services::worker::WorkerService;
use crate::services::worker_activator::WorkerActivator;
use crate::services::HasOplog;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AccountId, IdempotencyKey, OwnedWorkerId, ScheduleId, ScheduledAction};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::Value;
use std::ops::{Add, Deref};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{error, info, span, warn, Instrument, Level};

#[async_trait]
pub trait SchedulerService: Send + Sync {
    async fn schedule(&self, time: DateTime<Utc>, action: ScheduledAction) -> ScheduleId;

    async fn cancel(&self, id: ScheduleId);
}

/// A lighter trait than `WorkerActivator` that only provides the required functionality
/// for `SchedulerServiceDefault`, making it easier to test (by being independent of `WorkerCtx`).
#[async_trait]
pub trait SchedulerWorkerAccess {
    async fn activate_worker(&self, created_by: &AccountId, owned_worker_id: &OwnedWorkerId);
    async fn open_oplog(
        &self,
        created_by: &AccountId,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError>;

    // enqueue and invocation to the worker
    async fn enqueue_invocation(
        &self,
        created_by: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError>;
}

#[async_trait]
impl<Ctx: WorkerCtx> SchedulerWorkerAccess for Arc<dyn WorkerActivator<Ctx>> {
    async fn activate_worker(&self, created_by: &AccountId, owned_worker_id: &OwnedWorkerId) {
        self.deref()
            .activate_worker(created_by, owned_worker_id)
            .await;
    }

    async fn open_oplog(
        &self,
        created_by: &AccountId,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let worker = self
            .get_or_create_suspended(
                created_by,
                owned_worker_id,
                None,
                None,
                None,
                None,
                None,
                &InvocationContextStack::fresh(),
            )
            .await?;
        Ok(worker.oplog())
    }

    async fn enqueue_invocation(
        &self,
        created_by: &AccountId,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError> {
        let worker = self
            .get_or_create_suspended(
                created_by,
                owned_worker_id,
                None,
                None,
                None,
                None,
                None,
                &InvocationContextStack::fresh(),
            )
            .await?;

        worker
            .invoke(
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            )
            .await?;

        Worker::start_if_needed(worker).await?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct SchedulerServiceDefault {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shard_service: Arc<dyn ShardService>,
    promise_service: Arc<dyn PromiseService>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    oplog_service: Arc<dyn OplogService>,
    worker_service: Arc<dyn WorkerService>,
}

impl SchedulerServiceDefault {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        promise_service: Arc<dyn PromiseService>,
        worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
        oplog_service: Arc<dyn OplogService>,
        worker_service: Arc<dyn WorkerService>,
        process_interval: Duration,
    ) -> Arc<Self> {
        let svc = Self {
            key_value_storage,
            background_handle: Arc::new(Mutex::new(None)),
            shard_service,
            promise_service,
            oplog_service,
            worker_service,
            worker_access,
        };
        let svc = Arc::new(svc);
        let background_handle = {
            let svc = svc.clone();
            tokio::spawn(
                async move {
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
                }
                .in_current_span(),
            )
        };
        *svc.background_handle.lock().unwrap() = Some(background_handle);

        svc
    }

    async fn process(&self, now: DateTime<Utc>) -> Result<(), String> {
        let (hours_since_epoch, remainder) = Self::split_time(now);
        let previous_hours_since_epoch = hours_since_epoch - 1;

        let previous_hour_key = Self::schedule_key_from_timestamp(previous_hours_since_epoch);
        let current_hour_key = Self::schedule_key_from_timestamp(hours_since_epoch);

        // TODO: couple of issues with this implementation
        // 1: We only query scheduled actions for the current hour - 1. If we are unavailable for longer than that actions will not be run.
        // 2: We use the timestamp of the scheduled action as a unique key. If we have 2 actions scheduled for the same point in time one will be silently discarded.

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

        // ! Do not exist early from this loop because of failed actions, as it will cause all other actions to be skipped.
        // ! Errors will only be logged anyway, so just log them inline here and ignore.
        for (key, action) in matching {
            match action.clone() {
                ScheduledAction::CompletePromise {
                    account_id,
                    promise_id,
                    project_id,
                } => {
                    let owned_worker_id = OwnedWorkerId::new(&project_id, &promise_id.worker_id);

                    let result = self
                        .promise_service
                        .complete(promise_id.clone(), vec![])
                        .await;

                    // TODO: We probably need more error handling here as not completing a promise that is expected to complete can lead to deadlocks.
                    match result {
                        Ok(_) => {
                            // activate worker so it starts processing the newly completed promises
                            {
                                let span = span!(
                                    Level::INFO,
                                    "scheduler",
                                    worker_id = owned_worker_id.worker_id.to_string()
                                );

                                self.worker_access
                                    .activate_worker(&account_id, &owned_worker_id)
                                    .instrument(span)
                                    .await;
                            }

                            record_scheduled_promise_completed();
                        }
                        Err(e) => {
                            error!(
                                worker_id = owned_worker_id.to_string(),
                                promise_id = promise_id.to_string(),
                                "Failed to complete promise: {e}"
                            );
                        }
                    }
                }
                ScheduledAction::ArchiveOplog {
                    account_id,
                    owned_worker_id,
                    last_oplog_index,
                    next_after,
                } => {
                    if self.oplog_service.exists(&owned_worker_id).await {
                        let current_last_index =
                            self.oplog_service.get_last_index(&owned_worker_id).await;
                        if current_last_index == last_oplog_index {
                            // Need to create the `Worker` instance to avoid race conditions
                            match self
                                .worker_access
                                .open_oplog(&account_id, &owned_worker_id)
                                .await
                            {
                                Ok(oplog) => {
                                    let start = Instant::now();
                                    if let Some(more) = MultiLayerOplog::try_archive(&oplog).await {
                                        record_scheduled_archive(start.elapsed(), more);
                                        if more {
                                            self.schedule(
                                                now.add(next_after),
                                                ScheduledAction::ArchiveOplog {
                                                    account_id,
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
                                Err(error) => {
                                    error!(
                                        worker_id = owned_worker_id.to_string(),
                                        "Failed to activate worker for archiving: {error}"
                                    );
                                }
                            }
                        }

                        // TODO: metrics
                    }
                }
                ScheduledAction::Invoke {
                    account_id,
                    owned_worker_id,
                    idempotency_key,
                    full_function_name,
                    function_input,
                    invocation_context,
                } => {
                    // TODO: We probably need more error handling here and retry the action when we fail to enqueue the invocation.
                    // We don't really care that it completes here, but it needs to be persisted in the invocation queue.
                    let result = self
                        .worker_access
                        .enqueue_invocation(
                            &account_id,
                            &owned_worker_id,
                            idempotency_key,
                            full_function_name.clone(),
                            function_input,
                            invocation_context,
                        )
                        .await;

                    if let Err(e) = result {
                        error!(
                            worker_id = owned_worker_id.to_string(),
                            full_function_name = full_function_name,
                            "Failed to invoke worker with scheduled invocation: {e}"
                        );
                    };
                }
            }

            // We are completely done with the action, purge it from the queue
            self.key_value_storage
                .with_entity("scheduler", "process", "scheduled_action")
                .remove_from_sorted_set(KeyValueStorageNamespace::Schedule, key, &action)
                .await?;
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
        format!("worker:schedule:{timestamp}")
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
    use crate::services::golem_config::GolemConfig;
    use crate::services::oplog::{Oplog, OplogService, PrimaryOplogService};
    use crate::services::promise::PromiseServiceMock;
    use crate::services::scheduler::{
        SchedulerService, SchedulerServiceDefault, SchedulerWorkerAccess,
    };
    use crate::services::shard::{ShardService, ShardServiceDefault};
    use crate::services::worker::{DefaultWorkerService, WorkerService};
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use async_trait::async_trait;
    use bincode::Encode;
    use chrono::DateTime;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{
        AccountId, ComponentId, IdempotencyKey, OwnedWorkerId, ProjectId, PromiseId,
        ScheduledAction, ShardId, WorkerId,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_wasm::Value;
    use std::collections::{HashMap, HashSet};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use uuid::Uuid;

    struct SchedulerWorkerAccessMock;

    #[async_trait]
    impl SchedulerWorkerAccess for SchedulerWorkerAccessMock {
        async fn activate_worker(&self, _created_by: &AccountId, _owned_worker_id: &OwnedWorkerId) {
        }
        async fn open_oplog(
            &self,
            _created_by: &AccountId,
            _owned_worker_id: &OwnedWorkerId,
        ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
            unimplemented!()
        }
        async fn enqueue_invocation(
            &self,
            _created_by: &AccountId,
            _owned_worker_id: &OwnedWorkerId,
            _idempotency_key: IdempotencyKey,
            _full_function_name: String,
            _function_input: Vec<Value>,
            _invocation_context: InvocationContextStack,
        ) -> Result<(), WorkerExecutorError> {
            unimplemented!()
        }
    }

    fn serialized_bytes<T: Encode>(entry: &T) -> Vec<u8> {
        golem_common::serialization::serialize(entry)
            .expect("failed to serialize entry")
            .to_vec()
    }

    fn create_shard_service_mock() -> Arc<dyn ShardService> {
        let result = Arc::new(ShardServiceDefault::new());
        result.register(1, &HashSet::from_iter(vec![ShardId::new(0)]));
        result
    }

    fn create_promise_service_mock() -> Arc<PromiseServiceMock> {
        Arc::new(PromiseServiceMock::new())
    }

    fn create_worker_access_mock() -> Arc<dyn SchedulerWorkerAccess + Send + Sync> {
        Arc::new(SchedulerWorkerAccessMock)
    }

    async fn create_oplog_service_mock() -> Arc<dyn OplogService> {
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
        shard_service: Arc<dyn ShardService>,
        oplog_service: Arc<dyn OplogService>,
        config: Arc<GolemConfig>,
    ) -> Arc<dyn WorkerService> {
        Arc::new(DefaultWorkerService::new(
            kvs,
            shard_service,
            oplog_service,
            config,
        ))
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

        let project_id = ProjectId::new_v4();

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
        let worker_access = create_worker_access_mock();
        let oplog_service = create_oplog_service_mock().await;
        let golem_config = Arc::new(GolemConfig::default());
        let worker_service = create_worker_service_mock(
            kvs.clone(),
            shard_service.clone(),
            oplog_service.clone(),
            golem_config,
        );

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service,
            worker_access,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // not testing process() here
        );

        let account_id = AccountId {
            value: "test_account".to_string(),
        };

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    project_id: project_id.clone(),
                    promise_id: p1.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p2.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p3.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;

        let mut result = HashMap::new();
        kvs.sorted_sets()
            .iter_async(|key, entry| {
                result.insert(key.clone(), entry.clone());
                true
            })
            .await;
        assert_eq!(
            result,
            HashMap::from_iter(vec![
                (
                    "Schedule/worker:schedule:469329".to_string(),
                    vec![(
                        3540000.0,
                        serialized_bytes(&ScheduledAction::CompletePromise {
                            account_id: account_id.clone(),
                            promise_id: p2,
                            project_id: project_id.clone()
                        })
                    )]
                ),
                (
                    "Schedule/worker:schedule:469330".to_string(),
                    vec![
                        (
                            300000.0,
                            serialized_bytes(&ScheduledAction::CompletePromise {
                                account_id: account_id.clone(),
                                promise_id: p1,
                                project_id: project_id.clone()
                            })
                        ),
                        (
                            301000.0,
                            serialized_bytes(&ScheduledAction::CompletePromise {
                                account_id: account_id.clone(),
                                promise_id: p3,
                                project_id: project_id.clone()
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

        let project_id = ProjectId::new_v4();

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
        let worker_access = create_worker_access_mock();
        let oplog_service = create_oplog_service_mock().await;
        let golem_config = Arc::new(GolemConfig::default());

        let worker_service = create_worker_service_mock(
            kvs.clone(),
            shard_service.clone(),
            oplog_service.clone(),
            golem_config,
        );

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service,
            worker_access,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // not testing process() here
        );

        let account_id = AccountId {
            value: "test_account".to_string(),
        };

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p1.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p2.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p3.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;

        svc.cancel(s2).await;
        svc.cancel(s3).await;

        let mut result = HashMap::new();
        kvs.sorted_sets()
            .iter_async(|key, entry| {
                result.insert(key.clone(), entry.clone());
                true
            })
            .await;

        assert_eq!(
            result,
            HashMap::from([
                ("Schedule/worker:schedule:469329".to_string(), vec![]),
                (
                    "Schedule/worker:schedule:469330".to_string(),
                    vec![(
                        300000.0,
                        serialized_bytes(&ScheduledAction::CompletePromise {
                            account_id: account_id.clone(),
                            promise_id: p1,
                            project_id: project_id.clone()
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

        let project_id = ProjectId::new_v4();

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
        let worker_access = create_worker_access_mock();
        let oplog_service = create_oplog_service_mock().await;
        let golem_config = Arc::new(GolemConfig::default());
        let worker_service = create_worker_service_mock(
            kvs.clone(),
            shard_service.clone(),
            oplog_service.clone(),
            golem_config,
        );

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_access,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let account_id = AccountId {
            value: "test_account".to_string(),
        };

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p1.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p2.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p3.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let mut result = HashMap::new();
        kvs.sorted_sets()
            .iter_async(|key, entry| {
                result.insert(key.clone(), entry.clone());
                true
            })
            .await;
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([(
                "Schedule/worker:schedule:469330".to_string(),
                vec![(
                    3540000.0,
                    serialized_bytes(&ScheduledAction::CompletePromise {
                        account_id: account_id.clone(),
                        promise_id: p2.clone(),
                        project_id: project_id.clone()
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

        let project_id = ProjectId::new_v4();

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
        let worker_access = create_worker_access_mock();
        let oplog_service = create_oplog_service_mock().await;
        let golem_config = Arc::new(GolemConfig::default());
        let worker_service = create_worker_service_mock(
            kvs.clone(),
            shard_service.clone(),
            oplog_service.clone(),
            golem_config,
        );

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_access,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let account_id = AccountId {
            value: "test_account".to_string(),
        };

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p1.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p2.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p3.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let mut result = HashMap::new();
        kvs.sorted_sets()
            .iter_async(|key, entry| {
                result.insert(key.clone(), entry.clone());
                true
            })
            .await;
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

        let project_id = ProjectId::new_v4();

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
        let worker_access = create_worker_access_mock();
        let oplog_service = create_oplog_service_mock().await;
        let golem_config = Arc::new(GolemConfig::default());
        let worker_service = create_worker_service_mock(
            kvs.clone(),
            shard_service.clone(),
            oplog_service.clone(),
            golem_config,
        );

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_access,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let account_id = AccountId {
            value: "test_account".to_string(),
        };

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p1.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p2.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p3.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s4 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:47:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p4.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let mut result = HashMap::new();
        kvs.sorted_sets()
            .iter_async(|key, entry| {
                result.insert(key.clone(), entry.clone());
                true
            })
            .await;
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

        let project_id = ProjectId::new_v4();

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
        let worker_access = create_worker_access_mock();
        let oplog_service = create_oplog_service_mock().await;
        let golem_config = Arc::new(GolemConfig::default());
        let worker_service = create_worker_service_mock(
            kvs.clone(),
            shard_service.clone(),
            oplog_service.clone(),
            golem_config,
        );

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_access,
            oplog_service,
            worker_service,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let account_id = AccountId {
            value: "test_account".to_string(),
        };

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p1.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p2.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:47:00Z").unwrap(),
                ScheduledAction::CompletePromise {
                    account_id: account_id.clone(),
                    promise_id: p3.clone(),
                    project_id: project_id.clone(),
                },
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let mut result = HashMap::new();
        kvs.sorted_sets()
            .iter_async(|key, entry| {
                result.insert(key.clone(), entry.clone());
                true
            })
            .await;
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
