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

use chrono::{Duration as ChronoDuration, Utc};
use golem_common::config::DbPostgresConfig;
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentId, OwnedAgentId, PromiseId, ScheduleId, ScheduledAction, ShardAssignment, ShardId,
};
use golem_common::serialization::serialize;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_executor::services::golem_config::SchedulerStoragePostgresConfig;
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::postgres::PostgresSchedulerStorage;
use std::time::Duration;
use test_r::test;
use url::Url;
use uuid::Uuid;

#[test]
fn scheduled_action_fingerprints_roundtrip() {
    use golem_common::model::AgentFingerprint;
    use golem_common::serialization::deserialize;
    use std::io::Write;

    let account_id = AccountId(Uuid::from_u128(1));
    let environment_id = EnvironmentId(Uuid::from_u128(2));
    let agent_id = AgentId {
        component_id: ComponentId(Uuid::from_u128(3)),
        agent_id: "Counter(\"scheduled\")".to_string(),
    };
    let fingerprint = AgentFingerprint(Uuid::from_u128(4));
    let promise_id = PromiseId {
        agent_id: agent_id.clone(),
        fingerprint,
        oplog_idx: OplogIndex::from_u64(42),
    };
    let actions = [
        (
            "scheduled_action_complete_promise.bin",
            ScheduledAction::CompletePromise {
                account_id,
                environment_id,
                promise_id: promise_id.clone(),
                target_worker_fingerprint: fingerprint,
            },
        ),
        (
            "scheduled_action_resume.bin",
            ScheduledAction::Resume {
                agent_created_by: account_id,
                owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
                target_worker_fingerprint: fingerprint,
            },
        ),
    ];
    let mut mint = goldenfile::Mint::new("tests/goldenfiles");
    let encoded = serialize(&promise_id).unwrap();
    assert_eq!(deserialize::<PromiseId>(&encoded).unwrap(), promise_id);
    mint.new_goldenfile("promise_id.bin")
        .unwrap()
        .write_all(&encoded)
        .unwrap();
    for (name, action) in actions {
        let encoded = serialize(&action).unwrap();
        assert_eq!(deserialize::<ScheduledAction>(&encoded).unwrap(), action);
        mint.new_goldenfile(name)
            .unwrap()
            .write_all(&encoded)
            .unwrap();
    }
}

#[test]
async fn postgres_scheduler_storage_preserves_serialized_payload_and_idempotency() {
    let postgres = DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await;
    let db_name = format!("scheduler_{}", Uuid::new_v4().simple());
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&postgres.public_connection_string())
        .await
        .expect("Cannot create postgres admin pool");
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\";"))
        .execute(&admin_pool)
        .await
        .expect("Cannot create postgres test database");

    let config = SchedulerStoragePostgresConfig {
        postgres: DbPostgresConfig {
            host: "localhost".to_string(),
            database: db_name,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: Url::parse(&postgres.public_connection_string())
                .expect("Invalid postgres connection string")
                .port()
                .expect("Postgres connection string missing port"),
            max_connections: 10,
            schema: None,
            acquire_timeout: None,
        },
    };
    let storage = PostgresSchedulerStorage::configured(&config)
        .await
        .expect("Cannot create postgres scheduler storage");

    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "scheduled".to_string(),
    };
    let environment_id = EnvironmentId::new();
    let fingerprint = golem_common::model::AgentFingerprint::new();
    let action = ScheduledAction::CompletePromise {
        account_id: AccountId::new(),
        target_worker_fingerprint: fingerprint,
        environment_id,
        promise_id: PromiseId {
            agent_id: agent_id.clone(),
            fingerprint,
            oplog_idx: OplogIndex::from_u64(42),
        },
    };
    let conflicting_action = ScheduledAction::Resume {
        agent_created_by: AccountId::new(),
        target_worker_fingerprint: golem_common::model::AgentFingerprint::new(),
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
    };
    let schedule_id = ScheduleId::fresh();
    let shard_id = ShardId::new(0);
    let due_at = Utc::now() - ChronoDuration::seconds(1);

    storage
        .insert(schedule_id, due_at, shard_id, &serialize(&action).unwrap())
        .await
        .unwrap();
    storage
        .insert(
            schedule_id,
            due_at + ChronoDuration::minutes(1),
            ShardId::new(1),
            &serialize(&conflicting_action).unwrap(),
        )
        .await
        .unwrap();

    let assignment = ShardAssignment::unexpiring(2, [shard_id]);
    let claimed = storage
        .claim_due(Utc::now(), &assignment, 10, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].schedule_id, schedule_id);
    assert_eq!(claimed[0].action, action);
    assert!(
        storage
            .ack(&schedule_id, claimed[0].lease_owner)
            .await
            .unwrap()
    );
    assert_eq!(storage.count_due(Utc::now(), &assignment).await.unwrap(), 0);
}
