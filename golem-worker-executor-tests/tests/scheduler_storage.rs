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

use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentId, OwnedAgentId, PromiseId, ScheduleId, ScheduledAction, ShardAssignment, ShardId,
};
use golem_common::serialization::serialize;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_executor::services::golem_config::SchedulerStoragePostgresConfig;
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::memory::InMemorySchedulerStorage;
use golem_worker_executor::storage::scheduler::postgres::PostgresSchedulerStorage;
use golem_worker_executor::storage::scheduler::sqlite::SqliteSchedulerStorage;
use std::time::Duration;
use test_r::{test, timeout};
use url::Url;
use uuid::Uuid;

#[test]
fn scheduled_action_fingerprints_roundtrip() {
    use golem_common::model::AgentFingerprint;
    use golem_common::serialization::deserialize;
    use std::io::Write;

    let account_id = AccountId(Uuid::from_u128(1));
    let environment_id = EnvironmentId(Uuid::from_u128(2));
    let agent_id = AgentId::from_parts(7, 3);
    let fingerprint = AgentFingerprint(Uuid::from_u128(4));
    let promise_id = PromiseId {
        agent_id,
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
fn scheduled_invocation_wake_roundtrips_without_payload() {
    use golem_common::model::{AgentFingerprint, InvocationId};
    use golem_common::serialization::deserialize;

    let owned_agent_id = crate::fresh_owned_agent_id();
    let fingerprint = AgentFingerprint::new();
    let key = InvocationId(Uuid::new_v4());
    let action = ScheduledAction::WakeInvocation {
        owned_agent_id: owned_agent_id.clone(),
        idempotency_key: key,
        target_worker_fingerprint: fingerprint,
    };
    let decoded = deserialize::<ScheduledAction>(&serialize(&action).unwrap()).unwrap();
    assert_eq!(decoded, action);
    assert_eq!(decoded.owned_agent_id(), owned_agent_id);
    let id = ScheduleId::for_invocation(fingerprint, &key);
    assert_eq!(
        id.id,
        Uuid::new_v5(&fingerprint.0, key.to_string().as_bytes())
    );
    assert_eq!(id, ScheduleId::for_invocation(fingerprint, &key));
    assert_ne!(
        id,
        ScheduleId::for_invocation(AgentFingerprint::new(), &key)
    );
    assert_ne!(
        id,
        ScheduleId::for_invocation(fingerprint, &InvocationId(Uuid::new_v4()))
    );
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

    let agent_id = crate::fresh_owned_agent_id().agent_id;
    let environment_id = EnvironmentId::new();
    let idempotency_key = golem_common::model::InvocationId(Uuid::new_v4());
    let action = ScheduledAction::WakeInvocation {
        target_worker_fingerprint: golem_common::model::AgentFingerprint::new(),
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
        idempotency_key,
    };
    let conflicting_action = ScheduledAction::Resume {
        agent_created_by: AccountId::new(),
        target_worker_fingerprint: golem_common::model::AgentFingerprint::new(),
        owned_agent_id: OwnedAgentId::new(environment_id, &agent_id),
    };
    let schedule_id = ScheduleId::fresh();
    let shard_id = ShardId::new(0);
    let due_at = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(1);

    storage
        .insert(schedule_id, due_at, shard_id, &serialize(&action).unwrap())
        .await
        .unwrap();
    storage
        .insert(
            schedule_id,
            due_at + jiff::SignedDuration::from_mins(1),
            ShardId::new(1),
            &serialize(&conflicting_action).unwrap(),
        )
        .await
        .unwrap();

    let assignment = ShardAssignment::unexpiring(2, [shard_id]);
    let claimed = storage
        .claim_due(
            jiff::Timestamp::now(),
            &assignment,
            10,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].schedule_id, schedule_id);
    assert_eq!(claimed[0].due_at.as_millisecond(), due_at.as_millisecond());
    assert_eq!(claimed[0].action, action);
    assert!(
        storage
            .ack(&schedule_id, claimed[0].lease_owner, jiff::Timestamp::now())
            .await
            .unwrap()
    );
    assert_eq!(
        storage
            .count_due(jiff::Timestamp::now(), &assignment)
            .await
            .unwrap(),
        0
    );
}

fn lease_action() -> ScheduledAction {
    ScheduledAction::WakeInvocation {
        owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(11, 5)),
        idempotency_key: golem_common::model::InvocationId(Uuid::new_v4()),
        target_worker_fingerprint: golem_common::model::AgentFingerprint::new(),
    }
}

async fn assert_scheduler_lease_contracts(
    holder: &(dyn SchedulerStorage + Send + Sync),
    contender: &(dyn SchedulerStorage + Send + Sync),
) {
    let action = lease_action();
    let schedule_id = ScheduleId::fresh();
    let now = jiff::Timestamp::from_millisecond(1_700_000_000_000).unwrap();
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    holder
        .insert(
            schedule_id,
            now - jiff::SignedDuration::from_secs(2),
            ShardId::new(0),
            &serialize(&action).unwrap(),
        )
        .await
        .unwrap();

    // A lease issued in the past is already expired in both the supplied clock
    // and wall-clock time; no sleep or backend-private state change is needed.
    let claimed = holder
        .claim_due(
            now - jiff::SignedDuration::from_secs(1),
            &assignment,
            1,
            Duration::from_millis(100),
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].schedule_id, schedule_id);
    assert_eq!(claimed[0].action, action);
    assert_eq!(contender.count_due(now, &assignment).await.unwrap(), 1);

    assert!(
        !holder
            .ack(&schedule_id, claimed[0].lease_owner, now)
            .await
            .unwrap(),
        "an expired lease must not authorize ack, even before another executor reclaims it"
    );
    let reclaimed = contender
        .claim_due(now, &assignment, 1, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].schedule_id, schedule_id);
    assert_eq!(reclaimed[0].action, action);
    assert_ne!(reclaimed[0].lease_owner, claimed[0].lease_owner);
    assert_eq!(reclaimed[0].attempt_count, 2);
    assert!(
        contender
            .ack(&schedule_id, reclaimed[0].lease_owner, now)
            .await
            .unwrap()
    );
    assert_eq!(holder.count_due(now, &assignment).await.unwrap(), 0);

    let ttl = Duration::from_secs(10);
    let expiry = now + ttl;
    // Compare on both sides of the millisecond boundary, including timestamps
    // whose nanoseconds would otherwise make the backends disagree.
    for ack_at in [
        expiry - jiff::SignedDuration::from_nanos(1),
        expiry,
        expiry + jiff::SignedDuration::from_nanos(1),
        expiry + jiff::SignedDuration::from_millis(1),
    ] {
        let id = ScheduleId::fresh();
        holder
            .insert(id, now, ShardId::new(0), &serialize(&action).unwrap())
            .await
            .unwrap();
        let first = holder.claim_due(now, &assignment, 1, ttl).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].schedule_id, id);
        assert!(
            contender
                .claim_due(
                    expiry - jiff::SignedDuration::from_nanos(1),
                    &assignment,
                    1,
                    ttl
                )
                .await
                .unwrap()
                .is_empty()
        );
        let live = ack_at < expiry;
        assert_eq!(
            holder.ack(&id, first[0].lease_owner, ack_at).await.unwrap(),
            live
        );
        assert_eq!(
            contender.count_due(ack_at, &assignment).await.unwrap(),
            u64::from(!live)
        );
        if !live {
            assert!(
                !holder
                    .extend_lease(&id, first[0].lease_owner, expiry + ttl, ack_at)
                    .await
                    .unwrap()
            );
            let second = contender
                .claim_due(expiry, &assignment, 1, ttl)
                .await
                .unwrap();
            assert_eq!(second.len(), 1);
            assert_eq!(second[0].schedule_id, id);
            assert_eq!(second[0].action, action);
            assert_eq!(second[0].due_at, now);
            assert_eq!(second[0].attempt_count, 2);
            assert_ne!(first[0].lease_owner, second[0].lease_owner);
            // A past as-of time isolates token fencing from expiry fencing.
            assert!(!holder.ack(&id, first[0].lease_owner, now).await.unwrap());
            assert!(
                !holder
                    .extend_lease(&id, first[0].lease_owner, expiry + ttl + ttl, now)
                    .await
                    .unwrap()
            );
            assert!(
                contender
                    .extend_lease(&id, second[0].lease_owner, expiry + ttl + ttl, expiry)
                    .await
                    .unwrap()
            );
            assert!(
                holder
                    .claim_due(expiry + ttl, &assignment, 1, ttl)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(
                contender
                    .ack(&id, second[0].lease_owner, expiry + ttl)
                    .await
                    .unwrap()
            );
            assert_eq!(
                holder.count_due(expiry + ttl, &assignment).await.unwrap(),
                0
            );
        }
    }

    let id = ScheduleId::fresh();
    holder
        .insert(id, now, ShardId::new(0), &serialize(&action).unwrap())
        .await
        .unwrap();
    let first = holder.claim_due(now, &assignment, 1, ttl).await.unwrap();
    assert_eq!(first.len(), 1);
    let renew_at = now + jiff::SignedDuration::from_secs(1);
    for invalid_until in [
        renew_at - jiff::SignedDuration::from_millis(1),
        renew_at,
        renew_at + jiff::SignedDuration::from_nanos(1),
    ] {
        assert!(
            !holder
                .extend_lease(&id, first[0].lease_owner, invalid_until, renew_at)
                .await
                .unwrap()
        );
    }
    let extended = now + jiff::SignedDuration::from_secs(37);
    assert!(
        holder
            .extend_lease(&id, first[0].lease_owner, extended, renew_at)
            .await
            .unwrap()
    );
    assert!(
        holder
            .extend_lease(&id, first[0].lease_owner, expiry, renew_at)
            .await
            .unwrap()
    );
    assert!(
        contender
            .claim_due(expiry, &assignment, 1, ttl)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        contender
            .claim_due(
                extended - jiff::SignedDuration::from_nanos(1),
                &assignment,
                1,
                ttl
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !holder
            .ack(&id, first[0].lease_owner, extended)
            .await
            .unwrap()
    );
    assert!(
        !holder
            .extend_lease(&id, first[0].lease_owner, extended + ttl, extended)
            .await
            .unwrap()
    );
    let second = contender
        .claim_due(extended, &assignment, 1, ttl)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].schedule_id, id);
    assert_eq!(second[0].attempt_count, 2);
    assert_ne!(second[0].lease_owner, first[0].lease_owner);
    assert!(
        contender
            .ack(&id, second[0].lease_owner, extended)
            .await
            .unwrap()
    );
    assert_eq!(holder.count_due(extended, &assignment).await.unwrap(), 0);

    let id = ScheduleId::fresh();
    holder
        .insert(id, now, ShardId::new(0), &serialize(&action).unwrap())
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        holder.claim_due(now, &assignment, 1, ttl),
        contender.claim_due(now, &assignment, 1, ttl),
    );
    let mut claims = left.unwrap();
    claims.extend(right.unwrap());
    assert_eq!(
        claims.len(),
        1,
        "two connections must not claim the same live lease"
    );
    assert_eq!(claims[0].schedule_id, id);
    let (left, right) = tokio::join!(
        holder.claim_due(expiry, &assignment, 1, ttl),
        contender.claim_due(expiry, &assignment, 1, ttl),
    );
    let mut replacements = left.unwrap();
    replacements.extend(right.unwrap());
    assert_eq!(
        replacements.len(),
        1,
        "only one connection may reclaim an expired lease"
    );
    assert_eq!(replacements[0].schedule_id, id);
    assert_eq!(replacements[0].attempt_count, 2);
    assert_ne!(claims[0].lease_owner, replacements[0].lease_owner);
    assert!(!holder.ack(&id, claims[0].lease_owner, now).await.unwrap());
    assert!(
        !holder
            .extend_lease(&id, claims[0].lease_owner, expiry + ttl + ttl, now)
            .await
            .unwrap()
    );
    assert!(
        contender
            .ack(&id, replacements[0].lease_owner, expiry)
            .await
            .unwrap()
    );
    assert_eq!(holder.count_due(expiry, &assignment).await.unwrap(), 0);
    assert!(
        !holder
            .ack(&id, replacements[0].lease_owner, expiry)
            .await
            .unwrap()
    );
    assert!(
        !contender
            .extend_lease(&id, replacements[0].lease_owner, expiry + ttl, expiry)
            .await
            .unwrap()
    );
}

enum LeaseBackend {
    Sqlite(DbSqliteConfig),
    Postgres(SchedulerStoragePostgresConfig),
}

enum LeaseConnection {
    Sqlite(SqliteSchedulerStorage, SqlitePool),
    Postgres(PostgresSchedulerStorage, sqlx::PgPool),
}

impl LeaseBackend {
    async fn open(&self) -> LeaseConnection {
        match self {
            Self::Sqlite(config) => {
                let pool = SqlitePool::configured(config).await.unwrap();
                let storage = SqliteSchedulerStorage::new(pool.clone()).await.unwrap();
                LeaseConnection::Sqlite(storage, pool)
            }
            Self::Postgres(config) => {
                let pool = sqlx::postgres::PgPoolOptions::new()
                    .max_connections(1)
                    .connect_with(config.postgres.connect_options())
                    .await
                    .unwrap();
                let storage =
                    PostgresSchedulerStorage::new(PostgresPool::new(pool.clone()).await.unwrap())
                        .await
                        .unwrap();
                LeaseConnection::Postgres(storage, pool)
            }
        }
    }
}

impl LeaseConnection {
    fn storage(&self) -> &(dyn SchedulerStorage + Send + Sync) {
        match self {
            Self::Sqlite(storage, _) => storage,
            Self::Postgres(storage, _) => storage,
        }
    }

    async fn close(self) {
        match self {
            Self::Sqlite(storage, pool) => {
                drop(storage);
                pool.close().await;
            }
            Self::Postgres(storage, pool) => {
                drop(storage);
                pool.close().await;
            }
        }
    }
}

async fn assert_persistent_scheduler_lease_contracts(backend: LeaseBackend) {
    let holder = backend.open().await;
    let contender = backend.open().await;
    assert_scheduler_lease_contracts(holder.storage(), contender.storage()).await;

    let now = jiff::Timestamp::from_millisecond(1_700_000_000_000).unwrap();
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let action = lease_action();
    let mut leases = Vec::new();
    for extend in [false, true] {
        let id = ScheduleId::fresh();
        holder
            .storage()
            .insert(id, now, ShardId::new(0), &serialize(&action).unwrap())
            .await
            .unwrap();
        let claim = holder
            .storage()
            .claim_due(now, &assignment, 1, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(claim.len(), 1);
        assert_eq!(claim[0].schedule_id, id);
        let until = now + jiff::SignedDuration::from_secs(if extend { 41 } else { 10 });
        if extend {
            assert!(
                holder
                    .storage()
                    .extend_lease(&id, claim[0].lease_owner, until, now)
                    .await
                    .unwrap()
            );
        }
        leases.push((id, claim[0].lease_owner, until));
    }
    holder.close().await;
    contender.close().await;

    let reopened = backend.open().await;
    assert_eq!(
        reopened
            .storage()
            .count_due(now, &assignment)
            .await
            .unwrap(),
        2
    );
    assert!(
        reopened
            .storage()
            .claim_due(
                now + jiff::SignedDuration::from_secs(9),
                &assignment,
                2,
                Duration::from_secs(10)
            )
            .await
            .unwrap()
            .is_empty()
    );
    for (id, owner, until) in leases {
        let before_expiry = until - jiff::SignedDuration::from_nanos(1);
        assert!(
            reopened
                .storage()
                .claim_due(before_expiry, &assignment, 2, Duration::from_secs(10))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .storage()
                .extend_lease(&id, owner, until, before_expiry)
                .await
                .unwrap()
        );
        assert!(!reopened.storage().ack(&id, owner, until).await.unwrap());
        assert!(
            !reopened
                .storage()
                .extend_lease(
                    &id,
                    owner,
                    until + jiff::SignedDuration::from_secs(60),
                    until
                )
                .await
                .unwrap()
        );
        let claim = reopened
            .storage()
            .claim_due(until, &assignment, 2, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(claim.len(), 1);
        assert_eq!(claim[0].schedule_id, id);
        assert_eq!(claim[0].action, action);
        assert_eq!(claim[0].due_at, now);
        assert_eq!(claim[0].attempt_count, 2);
        assert_ne!(claim[0].lease_owner, owner);
        assert!(
            reopened
                .storage()
                .ack(&id, claim[0].lease_owner, until)
                .await
                .unwrap()
        );
    }
    assert_eq!(
        reopened
            .storage()
            .count_due(now, &assignment)
            .await
            .unwrap(),
        0
    );
    reopened.close().await;
}

#[test]
async fn in_memory_scheduler_storage_lease_contracts() {
    let storage = InMemorySchedulerStorage::new();
    assert_scheduler_lease_contracts(&storage, &storage).await;
}

#[test]
#[timeout("120s")]
async fn sqlite_scheduler_storage_lease_contracts() {
    let directory = tempfile::tempdir().unwrap();
    let config = DbSqliteConfig {
        database: directory
            .path()
            .join("scheduler.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 1,
        foreign_keys: false,
    };
    drop(SqliteSchedulerStorage::configured(&config).await.unwrap());
    assert_persistent_scheduler_lease_contracts(LeaseBackend::Sqlite(config)).await;
}

#[test]
#[timeout("120s")]
async fn postgres_scheduler_storage_lease_contracts() {
    let postgres = DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await;
    let url = Url::parse(&postgres.public_connection_string()).unwrap();
    let config = SchedulerStoragePostgresConfig {
        postgres: DbPostgresConfig {
            host: url.host_str().unwrap().to_string(),
            database: url.path().trim_start_matches('/').to_string(),
            username: url.username().to_string(),
            password: url.password().unwrap().to_string(),
            port: url.port().unwrap(),
            max_connections: 1,
            schema: None,
            acquire_timeout: None,
        },
    };
    drop(PostgresSchedulerStorage::configured(&config).await.unwrap());
    assert_persistent_scheduler_lease_contracts(LeaseBackend::Postgres(config)).await;
}
