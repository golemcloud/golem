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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use tokio::task::JoinHandle;
use tracing::{error, span, Instrument, Level};

use golem_common::model::{PromiseId, ScheduleId};

use crate::metrics::promises::record_scheduled_promise_completed;
use crate::services::promise::PromiseService;
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};

#[async_trait]
pub trait SchedulerService {
    async fn schedule(&self, time: DateTime<Utc>, promise_id: PromiseId) -> ScheduleId;

    async fn cancel(&self, id: ScheduleId);
}

#[derive(Clone)]
pub struct SchedulerServiceDefault {
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shard_service: Arc<dyn ShardService + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
}

impl SchedulerServiceDefault {
    pub fn new(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        process_interval: Duration,
    ) -> Arc<Self> {
        let svc = Self {
            key_value_storage,
            background_handle: Arc::new(Mutex::new(None)),
            shard_service,
            promise_service,
            worker_activator,
        };
        let svc = Arc::new(svc);
        let svc_clone = svc.clone();
        let background_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(process_interval).await;
                let r = svc_clone.process(Utc::now()).await;
                if let Err(err) = r {
                    error!("Error in scheduler background task: {}", err);
                }
            }
        });
        *svc.background_handle.lock().unwrap() = Some(background_handle);

        svc
    }

    async fn process(&self, now: DateTime<Utc>) -> Result<(), String> {
        let (hours_since_epoch, remainder) = Self::split_time(now);
        let previous_hours_since_epoch = hours_since_epoch - 1;

        let previous_hour_key = Self::schedule_key_from_timestamp(previous_hours_since_epoch);
        let current_hour_key = Self::schedule_key_from_timestamp(hours_since_epoch);

        let all_from_prev_hour: Vec<(f64, PromiseId)> = self
            .key_value_storage
            .with_entity("scheduler", "process", "promise_id")
            .get_sorted_set(KeyValueStorageNamespace::Schedule, &previous_hour_key)
            .await?;

        let mut all: Vec<(&str, PromiseId)> = all_from_prev_hour
            .into_iter()
            .map(|(_score, promise_id)| (previous_hour_key.as_str(), promise_id))
            .collect();

        let all_from_this_hour: Vec<(f64, PromiseId)> = self
            .key_value_storage
            .with_entity("scheduler", "process", "promise_id")
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
                .map(|(_score, promise_id)| (current_hour_key.as_str(), promise_id)),
        );

        let matching: Vec<(&str, PromiseId)> = all
            .into_iter()
            .filter(|(_, promise_id)| {
                self.shard_service
                    .check_worker(&promise_id.worker_id)
                    .is_ok()
            })
            .collect::<Vec<_>>();

        let mut worker_ids = HashSet::new();
        for (key, promise_id) in matching {
            worker_ids.insert(promise_id.worker_id.clone());
            self.key_value_storage
                .with_entity("scheduler", "process", "promise_id")
                .remove_from_sorted_set(KeyValueStorageNamespace::Schedule, key, &promise_id)
                .await?;
            self.promise_service
                .complete(promise_id, vec![])
                .await
                .map_err(|golem_err| format!("{golem_err}"))?;

            record_scheduled_promise_completed();
        }

        for worker_id in worker_ids {
            let span = span!(Level::INFO, "scheduler", worker_id = worker_id.to_string());
            self.worker_activator
                .activate_worker(&worker_id)
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
    async fn schedule(&self, time: DateTime<Utc>, promise_id: PromiseId) -> ScheduleId {
        let (hours_since_epoch, remainder) = Self::split_time(time);
        let id = ScheduleId {
            timestamp: hours_since_epoch,
            promise_id: promise_id.clone(),
        };

        self.key_value_storage
            .with_entity("scheduler", "schedule", "promise_id")
            .add_to_sorted_set(
                KeyValueStorageNamespace::Schedule,
                &Self::schedule_key(&id),
                remainder,
                &promise_id,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to add schedule for promise id {promise_id} in KV storage: {err}")
            });

        id
    }

    async fn cancel(&self, id: ScheduleId) {
        self.key_value_storage
            .with_entity("scheduler", "cancel", "promise_id")
            .remove_from_sorted_set(
                KeyValueStorageNamespace::Schedule,
                &Self::schedule_key(&id),
                &id.promise_id,
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove schedule for promise id {} from KV storage: {err}",
                    id.promise_id
                )
            });
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct SchedulerServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for SchedulerServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl SchedulerServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl SchedulerService for SchedulerServiceMock {
    async fn schedule(&self, _time: DateTime<Utc>, _promise_id: PromiseId) -> ScheduleId {
        unimplemented!()
    }

    async fn cancel(&self, _id: ScheduleId) {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use bincode::Encode;

    use chrono::DateTime;

    use uuid::Uuid;

    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{ComponentId, PromiseId, WorkerId};

    use crate::services::promise::PromiseServiceMock;
    use crate::services::scheduler::{SchedulerService, SchedulerServiceDefault};
    use crate::services::shard::ShardServiceMock;
    use crate::services::worker_activator::WorkerActivatorMock;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

    fn serialized_bytes<T: Encode>(entry: &T) -> Vec<u8> {
        golem_common::serialization::serialize(entry)
            .expect("failed to serialize entry")
            .to_vec()
    }

    #[tokio::test]
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

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service,
            worker_activator,
            Duration::from_secs(1000), // not testing process() here
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                p1.clone(),
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                p2.clone(),
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:01Z").unwrap(),
                p3.clone(),
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
                    ("Schedule/worker:schedule:469329".to_string(), vec![
                        (3540000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"worker_name\":\"inst1\"}},\"oplog_idx\":123}}").to_string())))]),
                    ("Schedule/worker:schedule:469330".to_string(), vec![
                        (300000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"worker_name\":\"inst1\"}},\"oplog_idx\":101}}").to_string()))),
                        (301000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"worker_name\":\"inst2\"}},\"oplog_idx\":1000}}").to_string())))])
                ])
        );
    }

    #[tokio::test]
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

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service,
            worker_activator,
            Duration::from_secs(1000), // not testing process() here
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                p1.clone(),
            )
            .await;
        let s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                p2.clone(),
            )
            .await;
        let s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:01Z").unwrap(),
                p3.clone(),
            )
            .await;

        svc.cancel(s2).await;
        svc.cancel(s3).await;

        let uuid = c1.0.to_string();

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            result,
            HashMap::from(
                [
                    ("Schedule/worker:schedule:469329".to_string(), vec![]),
                    ("Schedule/worker:schedule:469330".to_string(), vec![(300000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"worker_name\":\"inst1\"}},\"oplog_idx\":101}}").to_string())))])
                ]
            )
        );
    }

    #[tokio::test]
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

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                p1.clone(),
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:59:00Z").unwrap(),
                p2.clone(),
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                p3.clone(),
            )
            .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let uuid = c1.0.to_string();

        let result = kvs
            .sorted_sets()
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<HashMap<_, _>>();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from(
                [
                    ("Schedule/worker:schedule:469330".to_string(), vec![(3540000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"worker_name\":\"inst1\"}},\"oplog_idx\":123}}").to_string())))])
                ]
            )
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(!completed_promises.contains(&p2));
    }

    #[tokio::test]
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

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                p1.clone(),
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                p2.clone(),
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                p3.clone(),
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

    #[tokio::test]
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

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                p1.clone(),
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                p2.clone(),
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:11:01Z").unwrap(),
                p3.clone(),
            )
            .await;
        let _s4 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:47:00Z").unwrap(),
                p4.clone(),
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

    #[tokio::test]
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

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            kvs.clone(),
            shard_service,
            promise_service.clone(),
            worker_activator,
            Duration::from_secs(1000), // explicitly calling process for testing
        );

        let _s1 = svc
            .schedule(
                DateTime::from_str("2023-07-17T10:05:00Z").unwrap(),
                p1.clone(),
            )
            .await;
        let _s2 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:59:00Z").unwrap(),
                p2.clone(),
            )
            .await;
        let _s3 = svc
            .schedule(
                DateTime::from_str("2023-07-17T09:47:00Z").unwrap(),
                p3.clone(),
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
