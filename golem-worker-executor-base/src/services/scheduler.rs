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

use crate::metrics::promises::record_scheduled_promise_completed;
use crate::services::promise::PromiseService;
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, TimeZone, Utc};
use golem_common::model::{PromiseId, ScheduleId};
use golem_common::redis::RedisPool;
use tokio::task::JoinHandle;
use tracing::error;

#[async_trait]
pub trait SchedulerService {
    async fn schedule(&self, time: DateTime<Utc>, promise_id: PromiseId) -> ScheduleId;

    async fn cancel(&self, id: ScheduleId);
}

#[derive(Clone)]
pub struct SchedulerServiceDefault {
    redis: RedisPool,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shard_service: Arc<dyn ShardService + Send + Sync>,
    promise_service: Arc<dyn PromiseService + Send + Sync>,
    worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
}

const HOUR_IN_MILLIS: i64 = 1000 * 60 * 60;

fn split_time<Tz: TimeZone>(time: DateTime<Tz>) -> (i64, f64) {
    let millis = time.timestamp_millis();
    let hours_since_epoch = millis / HOUR_IN_MILLIS;
    let remainder = (millis % HOUR_IN_MILLIS) as f64;
    (hours_since_epoch, remainder)
}

impl SchedulerServiceDefault {
    pub fn new(
        redis: RedisPool,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        process_interval: Duration,
    ) -> Arc<Self> {
        let svc = Self {
            redis,
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
        let (hours_since_epoch, remainder) = split_time(now);
        let previous_hours_since_epoch = hours_since_epoch - 1;

        let previous_hour_key = get_schedule_redis_key_from_timestamp(previous_hours_since_epoch);
        let current_hour_key = get_schedule_redis_key_from_timestamp(hours_since_epoch);

        let all_from_prev_hour_raw: Vec<Bytes> = self
            .redis
            .with("scheduler", "process")
            .zrange(&previous_hour_key, 0, -1, None, false, None, false)
            .await
            .map_err(|redis_err| format!("{redis_err}"))?;

        let all_from_prev_hour: Vec<(&str, PromiseId)> = all_from_prev_hour_raw
            .iter()
            .map(|serialized| {
                (
                    previous_hour_key.as_str(),
                    self.redis
                        .deserialize(serialized)
                        .expect("failed to deserialize worker id"),
                )
            })
            .collect();

        let all_from_this_hour_raw: Vec<Bytes> = self
            .redis
            .with("scheduler", "process")
            .zrangebyscore(&current_hour_key, 0.0, remainder, false, None)
            .await
            .map_err(|redis_err| format!("{redis_err}"))?;

        let mut all_from_this_hour: Vec<(&str, PromiseId)> = all_from_this_hour_raw
            .iter()
            .map(|serialized| {
                (
                    current_hour_key.as_str(),
                    self.redis
                        .deserialize(serialized)
                        .expect("failed to deserialize worker id"),
                )
            })
            .collect();

        let mut all = all_from_prev_hour;
        all.append(&mut all_from_this_hour);
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
            self.redis
                .with("scheduler", "process")
                .zrem(
                    key,
                    self.redis
                        .serialize(&promise_id)
                        .expect("failed to serialize promise id"),
                )
                .await
                .map_err(|redis_err| format!("{redis_err}"))?;
            self.promise_service
                .complete(promise_id, vec![])
                .await
                .map_err(|golem_err| format!("{golem_err}"))?;

            record_scheduled_promise_completed();
        }

        for worker_id in worker_ids {
            self.worker_activator.activate_worker(&worker_id).await;
        }

        Ok(())
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
        let (hours_since_epoch, remainder) = split_time(time);
        let id = ScheduleId {
            timestamp: hours_since_epoch,
            promise_id: promise_id.clone(),
        };
        let key = get_schedule_redis_key(&id);
        let value = self
            .redis
            .serialize(&promise_id)
            .expect("failed to serialize promise id");

        let _: u32 = self
            .redis
            .with("scheduler", "schedule")
            .zadd(key, None, None, false, false, (remainder, value))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to add schedule for promise id {promise_id} in Redis: {err}")
            });

        id
    }

    async fn cancel(&self, id: ScheduleId) {
        let key = get_schedule_redis_key(&id);
        let value = self
            .redis
            .serialize(&id.promise_id)
            .expect("failed to serialize promise id");
        let _: u32 = self
            .redis
            .with("scheduler", "cancel")
            .zrem(key, value)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to remove schedule for promise id {} from Redis: {err}",
                    id.promise_id
                )
            });
    }
}

fn get_schedule_redis_key(id: &ScheduleId) -> String {
    get_schedule_redis_key_from_timestamp(id.timestamp)
}

fn get_schedule_redis_key_from_timestamp(timestamp: i64) -> String {
    format!("instance:schedule:{}", timestamp)
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
    use std::cmp::{max, min};
    use std::collections::{HashMap, VecDeque};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::services::promise::PromiseServiceMock;
    use crate::services::shard::ShardServiceMock;
    use crate::services::worker_activator::WorkerActivatorMock;
    use bincode::Encode;
    use bytes::Bytes;
    use chrono::DateTime;
    use fred::error::RedisError;
    use fred::mocks::{MockCommand, Mocks};
    use fred::prelude::RedisValue;
    use golem_common::model::{PromiseId, TemplateId, WorkerId};
    use golem_common::redis::RedisPool;
    use uuid::Uuid;

    use crate::services::scheduler::{SchedulerService, SchedulerServiceDefault};

    #[derive(Debug)]
    pub struct RedisMock {
        commands: Mutex<VecDeque<MockCommand>>,
        data: Mutex<HashMap<String, Vec<(f64, Bytes)>>>,
    }

    impl RedisMock {
        pub fn new() -> Self {
            RedisMock {
                commands: Mutex::new(VecDeque::new()),
                data: Mutex::new(HashMap::new()),
            }
        }

        /// Drain and return the internal command buffer.
        pub fn take(&self) -> Vec<MockCommand> {
            self.commands.lock().unwrap().drain(..).collect()
        }

        /// Push a new command onto the back of the internal buffer.
        pub fn push_back(&self, command: MockCommand) {
            self.commands.lock().unwrap().push_back(command);
        }

        pub fn result(&self) -> HashMap<String, Vec<(f64, Bytes)>> {
            self.data.lock().unwrap().clone()
        }

        fn zadd(&self, args: Vec<RedisValue>) -> Result<RedisValue, RedisError> {
            let key = args.first().unwrap().as_string().unwrap();
            let score = args.get(1).unwrap().as_f64().unwrap();
            let value = args.get(2).unwrap().as_bytes().unwrap().to_vec();

            self.data
                .lock()
                .unwrap()
                .entry(key)
                .or_default()
                .push((score, Bytes::from(value)));
            Ok(RedisValue::Integer(0))
        }

        fn zrem(&self, args: Vec<RedisValue>) -> Result<RedisValue, RedisError> {
            let key = args.first().unwrap().as_string().unwrap();
            let value = args.get(1).unwrap().as_bytes().unwrap();
            self.data.lock().unwrap().entry(key).and_modify(|v| {
                v.retain(|(_, v)| *v != value);
            });
            Ok(RedisValue::Integer(0))
        }

        fn zrange(&self, args: Vec<RedisValue>) -> Result<RedisValue, RedisError> {
            let key = args.first().unwrap().as_string().unwrap();
            let from = args.get(1).unwrap().as_i64().unwrap() as usize;
            let to = args.get(2).unwrap().as_i64().unwrap();
            let empty = vec![];
            let binding = self.data.lock().unwrap();
            let all_items = binding.get(&key).unwrap_or(&empty);
            let to = min(
                all_items.len(),
                if to < 0 {
                    max(0, all_items.len() as i64 + to + 1) as usize
                } else {
                    to as usize
                },
            );
            // let result: Vec<RedisValue> =
            // all_items[from..to].iter().map(|(_, v)| RedisValue::Bytes(Bytes::copy_from_slice(v))).collect();
            let result: Vec<RedisValue> = all_items[from..to]
                .iter()
                .map(|(_, v)| RedisValue::Bytes(v.clone()))
                .collect();
            Ok(RedisValue::Array(result))
        }

        fn zrangebyscore(&self, args: Vec<RedisValue>) -> Result<RedisValue, RedisError> {
            let key = args.first().unwrap().as_string().unwrap();
            let from = args.get(1).unwrap().as_f64().unwrap();
            let to = args.get(2).unwrap().as_f64().unwrap();
            let empty = vec![];
            let binding = self.data.lock().unwrap();
            let all_items = binding.get(&key).unwrap_or(&empty);
            let mut result = Vec::new();
            for (score, value) in all_items {
                if score >= &from && score <= &to {
                    result.push(value.as_ref().into());
                }
            }
            Ok(RedisValue::Array(result))
        }
    }

    impl Mocks for RedisMock {
        fn process_command(&self, command: MockCommand) -> Result<RedisValue, RedisError> {
            self.push_back(command.clone());
            match &*command.cmd {
                "ZADD" => self.zadd(command.args),
                "ZREM" => self.zrem(command.args),
                "ZRANGE" => self.zrange(command.args),
                "ZRANGEBYSCORE" => self.zrangebyscore(command.args),
                _ => Ok(RedisValue::Queued),
            }
        }
    }

    fn serialized_data<T: Encode>(entry: &T) -> RedisValue {
        serialized_bytes(entry).into()
    }

    fn serialized_bytes<T: Encode>(entry: &T) -> Bytes {
        golem_common::serialization::serialize(entry).expect("failed to serialize entry")
    }

    #[cfg(test)]
    pub async fn mocked(mocks: Arc<dyn Mocks>) -> RedisPool {
        let config = fred::prelude::RedisConfig {
            mocks: Some(mocks),
            ..fred::prelude::RedisConfig::default()
        };
        let pool = fred::prelude::RedisPool::new(config, None, None, None, 1).unwrap();
        let pool = RedisPool::new(pool, "".to_string());
        pool.with("scheduler", "mocked")
            .ensure_connected()
            .await
            .unwrap();

        pool
    }

    #[tokio::test]
    pub async fn promises_added_to_expected_buckets() {
        let c1: TemplateId = TemplateId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 101,
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 123,
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: 1000,
        };

        let buffer = Arc::new(RedisMock::new());
        let pool = mocked(buffer.clone()).await;

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            pool,
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

        let cmds = buffer.take();
        let uuid = c1.0.to_string();
        assert_eq!(cmds, vec![
            MockCommand { cmd: "ZADD".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), 300000.0.into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":101}}")))] },
            MockCommand { cmd: "ZADD".into(), subcommand: None, args: vec!["instance:schedule:469329".as_bytes().into(), 3540000.0.into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":123}}")))] },
            MockCommand { cmd: "ZADD".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), 301000.0.into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst2\"}},\"oplog_idx\":1000}}")))] },
        ]);

        let result = buffer.result();
        assert_eq!(
            result,
            HashMap::from(
                [
                    ("instance:schedule:469329".to_string(), vec![
                        (3540000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":123}}").to_string())))]),
                    ("instance:schedule:469330".to_string(), vec![
                        (300000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":101}}").to_string()))),
                        (301000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst2\"}},\"oplog_idx\":1000}}").to_string())))])
                ]
            )
        );
    }

    #[tokio::test]
    pub async fn cancel_removes_entry() {
        let c1: TemplateId = TemplateId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 101,
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 123,
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: 1000,
        };

        let buffer = Arc::new(RedisMock::new());
        let pool = mocked(buffer.clone()).await;

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            pool,
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

        let cmds = buffer.take();
        let uuid = c1.0.to_string();

        assert_eq!(
            cmds,
            vec![
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        300000.0.into(),
                        serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":101}}")))],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        3540000.0.into(),
                        serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":123}}"))),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        301000.0.into(),
                        serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst2\"}},\"oplog_idx\":1000}}"))),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":123}}"))),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst2\"}},\"oplog_idx\":1000}}"))),
                    ],
                },
            ]
        );

        let result = buffer.result();
        assert_eq!(
            result,
            HashMap::from(
                [
                    ("instance:schedule:469329".to_string(), vec![]),
                    ("instance:schedule:469330".to_string(), vec![(300000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":101}}").to_string())))])
                ]
            )
        );
    }

    #[tokio::test]
    pub async fn process_current_hours_past_schedules() {
        let c1: TemplateId = TemplateId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 101,
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 123,
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: 1000,
        };

        let buffer = Arc::new(RedisMock::new());
        let pool = mocked(buffer.clone()).await;

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            pool,
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

        let cmds = buffer.take();
        let uuid = c1.0.to_string();
        assert_eq!(
            cmds,
            vec![
                MockCommand { cmd: "ZADD".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), 300000.0.into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":101}}")))] },
                MockCommand { cmd: "ZADD".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), 3540000.0.into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":123}}")))] },
                MockCommand { cmd: "ZADD".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), 661000.0.into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst2\"}},\"oplog_idx\":1000}}")))] },
                MockCommand { cmd: "ZRANGE".into(), subcommand: None, args: vec!["instance:schedule:469329".as_bytes().into(), 0.into(), (-1).into()] },
                MockCommand { cmd: "ZRANGEBYSCORE".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), 0.0.into(), 900000.0.into()] },
                MockCommand { cmd: "ZREM".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":101}}")))] },
                MockCommand { cmd: "ZREM".into(), subcommand: None, args: vec!["instance:schedule:469330".as_bytes().into(), serialized_data(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst2\"}},\"oplog_idx\":1000}}")))] },
            ]
        );

        let result = buffer.result();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from(
                [
                    ("instance:schedule:469330".to_string(), vec![(3540000.0, serialized_bytes(&PromiseId::from_json_string(&format!("{{\"instance_id\":{{\"component_id\":\"{uuid}\",\"instance_name\":\"inst1\"}},\"oplog_idx\":123}}").to_string())))])
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
        let c1: TemplateId = TemplateId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 101,
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 123,
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: 1000,
        };

        let buffer = Arc::new(RedisMock::new());
        let pool = mocked(buffer.clone()).await;

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            pool,
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

        let cmds = buffer.take();

        assert_eq!(
            cmds,
            vec![
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        300000.0.into(),
                        serialized_data(&p1),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        3540000.0.into(),
                        serialized_data(&p2),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        661000.0.into(),
                        serialized_data(&p3),
                    ],
                },
                MockCommand {
                    cmd: "ZRANGE".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        0.into(),
                        (-1).into(),
                    ],
                },
                MockCommand {
                    cmd: "ZRANGEBYSCORE".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        0.0.into(),
                        900000.0.into(),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        serialized_data(&p2),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        serialized_data(&p1),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        serialized_data(&p3),
                    ],
                },
            ]
        );

        let result = buffer.result();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([
                ("instance:schedule:469329".to_string(), vec![]),
                ("instance:schedule:469330".to_string(), vec![])
            ])
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(completed_promises.contains(&p2));
    }

    #[tokio::test]
    pub async fn process_past_and_current_hours_past_schedules_2() {
        let c1: TemplateId = TemplateId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 101,
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 123,
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: 1000,
        };
        let p4: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 111,
        };

        let buffer = Arc::new(RedisMock::new());
        let pool = mocked(buffer.clone()).await;

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            pool,
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

        let cmds = buffer.take();

        assert_eq!(
            cmds,
            vec![
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        300000.0.into(),
                        serialized_data(&p1),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        3540000.0.into(),
                        serialized_data(&p2),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        661000.0.into(),
                        serialized_data(&p3),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        2820000.0.into(),
                        serialized_data(&p4),
                    ],
                },
                MockCommand {
                    cmd: "ZRANGE".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        0.into(),
                        (-1).into(),
                    ],
                },
                MockCommand {
                    cmd: "ZRANGEBYSCORE".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        0.0.into(),
                        900000.0.into(),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        serialized_data(&p2),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        serialized_data(&p4),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        serialized_data(&p1),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        serialized_data(&p3),
                    ],
                },
            ]
        );

        let result = buffer.result();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([
                ("instance:schedule:469329".to_string(), vec![]),
                ("instance:schedule:469330".to_string(), vec![])
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
        let c1: TemplateId = TemplateId(Uuid::new_v4());
        let i1: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst1".to_string(),
        };
        let i2: WorkerId = WorkerId {
            template_id: c1.clone(),
            worker_name: "inst2".to_string(),
        };

        let p1: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 101,
        };
        let p2: PromiseId = PromiseId {
            worker_id: i1.clone(),
            oplog_idx: 123,
        };
        let p3: PromiseId = PromiseId {
            worker_id: i2.clone(),
            oplog_idx: 1000,
        };
        let buffer = Arc::new(RedisMock::new());
        let pool = mocked(buffer.clone()).await;

        let shard_service = Arc::new(ShardServiceMock::new());
        let promise_service = Arc::new(PromiseServiceMock::new());
        let worker_activator = Arc::new(WorkerActivatorMock::new());

        let svc = SchedulerServiceDefault::new(
            pool,
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

        let cmds = buffer.take();

        assert_eq!(
            cmds,
            vec![
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        300000.0.into(),
                        serialized_data(&p1),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        3540000.0.into(),
                        serialized_data(&p2),
                    ],
                },
                MockCommand {
                    cmd: "ZADD".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        2820000.0.into(),
                        serialized_data(&p3),
                    ],
                },
                MockCommand {
                    cmd: "ZRANGE".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        0.into(),
                        (-1).into(),
                    ],
                },
                MockCommand {
                    cmd: "ZRANGEBYSCORE".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        0.0.into(),
                        900000.0.into(),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        serialized_data(&p2),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469329".as_bytes().into(),
                        serialized_data(&p3),
                    ],
                },
                MockCommand {
                    cmd: "ZREM".into(),
                    subcommand: None,
                    args: vec![
                        "instance:schedule:469330".as_bytes().into(),
                        serialized_data(&p1),
                    ],
                },
            ]
        );

        let result = buffer.result();
        // The only item remaining is the one in the future
        assert_eq!(
            result,
            HashMap::from([
                ("instance:schedule:469329".to_string(), vec![]),
                ("instance:schedule:469330".to_string(), vec![])
            ])
        );

        let completed_promises = promise_service.all_completed().await;

        assert!(completed_promises.contains(&p1));
        assert!(completed_promises.contains(&p3));
        assert!(completed_promises.contains(&p2));
    }
}
