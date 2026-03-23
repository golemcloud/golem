// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source Available License v1.0 (the "License");
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

//! Service-layer tests for IgniteType / golem:rdbms/ignite2.
//!
//! An Apache Ignite 2.x container is started automatically via Docker
//! (testcontainers). No local Ignite installation or environment variables
//! are required.
//!
//! Run:
//!   cargo test -p golem-worker-executor --test integration ignite_service \
//!     -- --nocapture

use golem_common::model::component::ComponentId;
use golem_common::model::AgentId;
use golem_test_framework::components::rdb::docker_ignite::DockerIgniteRdb;
use golem_worker_executor::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
use golem_worker_executor::services::rdbms::ignite::{types as ignite_types, IgniteType};
use golem_worker_executor::services::rdbms::{
    Rdbms, RdbmsService, RdbmsServiceDefault, RdbmsTransactionStatus,
};
use std::sync::Arc;
use test_r::{test, test_dep};
use uuid::Uuid;

// ── test dependencies ─────────────────────────────────────────────────────────

#[test_dep]
async fn ignite_rdb() -> DockerIgniteRdb {
    DockerIgniteRdb::new().await
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_rdbms_service() -> RdbmsServiceDefault {
    RdbmsServiceDefault::new(RdbmsConfig {
        pool: RdbmsPoolConfig {
            max_connections: 10,
            ..Default::default()
        },
        ..Default::default()
    })
}

fn new_worker_id() -> AgentId {
    AgentId {
        component_id: ComponentId::new(),
        agent_id: format!("ignite-test-{}", Uuid::new_v4()),
    }
}

fn ignite() -> Arc<dyn Rdbms<IgniteType> + Send + Sync> {
    make_rdbms_service().ignite()
}

// ── basic connection test ─────────────────────────────────────────────────────

#[test]
async fn ignite_connection_test(ignite_rdb: &DockerIgniteRdb) {
    let url = ignite_rdb.connection_url();
    let rdbms = ignite();
    let worker_id = new_worker_id();

    let key = rdbms
        .create(&url, &worker_id)
        .await
        .expect("connect to Ignite");

    let exists = rdbms.exists(&key, &worker_id).await;
    assert!(exists, "connection should exist after create()");

    let _ = rdbms.remove(&key, &worker_id).await;
    let exists = rdbms.exists(&key, &worker_id).await;
    assert!(!exists, "connection should be gone after remove()");
}

// ── bad URL → query error (IgniteClient connects lazily) ─────────────────────

#[test]
async fn ignite_connection_err_test() {
    let rdbms = ignite();
    let worker_id = new_worker_id();

    // IgniteClient::new() is lazy — create() always succeeds.
    // The TCP connection is made on the first query, so that is what must fail.
    let key = rdbms
        .create("ignite://127.0.0.1:19999", &worker_id)
        .await
        .expect("create() is lazy, always succeeds");

    let result = rdbms
        .query(&key, &worker_id, "SELECT 1", vec![])
        .await;
    assert!(result.is_err(), "query to closed port must fail");
}

// ── create / insert / select ──────────────────────────────────────────────────

#[test]
async fn ignite_create_insert_select_test(ignite_rdb: &DockerIgniteRdb) {
    let url = ignite_rdb.connection_url();
    let rdbms = ignite();
    let worker_id = new_worker_id();

    let key = rdbms
        .create(&url, &worker_id)
        .await
        .expect("connect to Ignite");

    // unique table name per test run to avoid collisions
    let table = format!("test_users_{}", Uuid::new_v4().simple());

    // CREATE
    rdbms
        .execute(
            &key,
            &worker_id,
            &format!(
                "CREATE TABLE {table} \
                 (id INT PRIMARY KEY, name VARCHAR(200), score DOUBLE) \
                 WITH \"CACHE_NAME={table}\""
            ),
            vec![],
        )
        .await
        .expect("CREATE TABLE");

    // INSERT two rows
    for (id, name, score) in [(1i32, "Alice", 9.5_f64), (2, "Bob", 7.0_f64)] {
        rdbms
            .execute(
                &key,
                &worker_id,
                &format!("INSERT INTO {table} (id, name, score) VALUES (?, ?, ?)"),
                vec![
                    ignite_types::DbValue::Int(id),
                    ignite_types::DbValue::String(name.to_string()),
                    ignite_types::DbValue::Double(score),
                ],
            )
            .await
            .expect("INSERT");
    }

    // SELECT all
    let result = rdbms
        .query(
            &key,
            &worker_id,
            &format!("SELECT id, name, score FROM {table} ORDER BY id"),
            vec![],
        )
        .await
        .expect("SELECT");

    assert_eq!(result.rows.len(), 2, "expected 2 rows");

    // first row
    let row0 = &result.rows[0];
    assert_eq!(row0.values[0], ignite_types::DbValue::Int(1));
    assert_eq!(
        row0.values[1],
        ignite_types::DbValue::String("Alice".to_string())
    );
    assert_eq!(row0.values[2], ignite_types::DbValue::Double(9.5));

    // SELECT with parameter
    let result = rdbms
        .query(
            &key,
            &worker_id,
            &format!("SELECT name FROM {table} WHERE id = ?"),
            vec![ignite_types::DbValue::Int(2)],
        )
        .await
        .expect("SELECT with param");

    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].values[0],
        ignite_types::DbValue::String("Bob".to_string())
    );

    // cleanup
    rdbms
        .execute(&key, &worker_id, &format!("DROP TABLE {table}"), vec![])
        .await
        .ok();

    let _ = rdbms.remove(&key, &worker_id).await;
}

// ── transaction: commit ───────────────────────────────────────────────────────

#[test]
async fn ignite_transaction_commit_test(ignite_rdb: &DockerIgniteRdb) {
    let url = ignite_rdb.connection_url();
    let rdbms = ignite();
    let worker_id = new_worker_id();
    let key = rdbms.create(&url, &worker_id).await.expect("connect");

    let table = format!("txn_test_{}", Uuid::new_v4().simple());

    rdbms
        .execute(
            &key,
            &worker_id,
            &format!(
                "CREATE TABLE {table} (id INT PRIMARY KEY, val VARCHAR(100)) \
                 WITH \"CACHE_NAME={table},ATOMICITY=TRANSACTIONAL\""
            ),
            vec![],
        )
        .await
        .expect("CREATE TABLE");

    let txn = rdbms
        .begin_transaction(&key, &worker_id)
        .await
        .expect("begin transaction");

    let txn_id = txn.transaction_id();

    txn.execute(
        &format!("INSERT INTO {table} (id, val) VALUES (?, ?)"),
        vec![
            ignite_types::DbValue::Int(42),
            ignite_types::DbValue::String("committed".to_string()),
        ],
    )
    .await
    .expect("INSERT in txn");

    txn.pre_commit().await.expect("pre_commit");
    txn.commit().await.expect("commit");

    let status = rdbms
        .get_transaction_status(&key, &worker_id, &txn_id)
        .await
        .expect("get_transaction_status");
    assert_eq!(status, RdbmsTransactionStatus::Committed);

    // row should be visible after commit
    let result = rdbms
        .query(
            &key,
            &worker_id,
            &format!("SELECT val FROM {table} WHERE id = ?"),
            vec![ignite_types::DbValue::Int(42)],
        )
        .await
        .expect("SELECT after commit");
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].values[0],
        ignite_types::DbValue::String("committed".to_string())
    );

    rdbms
        .execute(&key, &worker_id, &format!("DROP TABLE {table}"), vec![])
        .await
        .ok();
    let _ = rdbms.remove(&key, &worker_id).await;
}

// ── transaction: rollback ─────────────────────────────────────────────────────

#[test]
async fn ignite_transaction_rollback_test(ignite_rdb: &DockerIgniteRdb) {
    let url = ignite_rdb.connection_url();
    let rdbms = ignite();
    let worker_id = new_worker_id();
    let key = rdbms.create(&url, &worker_id).await.expect("connect");

    let table = format!("txn_rb_{}", Uuid::new_v4().simple());

    rdbms
        .execute(
            &key,
            &worker_id,
            &format!(
                "CREATE TABLE {table} (id INT PRIMARY KEY, val VARCHAR(100)) \
                 WITH \"CACHE_NAME={table},ATOMICITY=TRANSACTIONAL\""
            ),
            vec![],
        )
        .await
        .expect("CREATE TABLE");

    let txn = rdbms
        .begin_transaction(&key, &worker_id)
        .await
        .expect("begin transaction");

    let txn_id = txn.transaction_id();

    txn.execute(
        &format!("INSERT INTO {table} (id, val) VALUES (?, ?)"),
        vec![
            ignite_types::DbValue::Int(99),
            ignite_types::DbValue::String("should_rollback".to_string()),
        ],
    )
    .await
    .expect("INSERT in txn");

    txn.pre_rollback().await.expect("pre_rollback");
    txn.rollback().await.expect("rollback");

    let status = rdbms
        .get_transaction_status(&key, &worker_id, &txn_id)
        .await
        .expect("get_transaction_status");
    assert_eq!(status, RdbmsTransactionStatus::RolledBack);

    // row should NOT be visible after rollback
    let result = rdbms
        .query(
            &key,
            &worker_id,
            &format!("SELECT val FROM {table} WHERE id = ?"),
            vec![ignite_types::DbValue::Int(99)],
        )
        .await
        .expect("SELECT after rollback");
    assert_eq!(result.rows.len(), 0, "rolled-back row must not be visible");

    rdbms
        .execute(&key, &worker_id, &format!("DROP TABLE {table}"), vec![])
        .await
        .ok();
    let _ = rdbms.remove(&key, &worker_id).await;
}

// ── query stream ──────────────────────────────────────────────────────────────

#[test]
async fn ignite_query_stream_test(ignite_rdb: &DockerIgniteRdb) {
    let url = ignite_rdb.connection_url();
    let rdbms = ignite();
    let worker_id = new_worker_id();
    let key = rdbms.create(&url, &worker_id).await.expect("connect");

    let table = format!("stream_test_{}", Uuid::new_v4().simple());

    rdbms
        .execute(
            &key,
            &worker_id,
            &format!(
                "CREATE TABLE {table} (id INT PRIMARY KEY, val INT) \
                 WITH \"CACHE_NAME={table}\""
            ),
            vec![],
        )
        .await
        .expect("CREATE TABLE");

    for i in 0..20i32 {
        rdbms
            .execute(
                &key,
                &worker_id,
                &format!("INSERT INTO {table} (id, val) VALUES (?, ?)"),
                vec![
                    ignite_types::DbValue::Int(i),
                    ignite_types::DbValue::Int(i * 10),
                ],
            )
            .await
            .expect("INSERT");
    }

    let stream = rdbms
        .query_stream(
            &key,
            &worker_id,
            &format!("SELECT id, val FROM {table} ORDER BY id"),
            vec![],
        )
        .await
        .expect("query_stream");

    let mut total = 0usize;
    loop {
        let batch = stream.get_next().await.expect("get_next");
        match batch {
            Some(rows) => total += rows.len(),
            None => break,
        }
    }
    assert_eq!(total, 20, "stream should yield all 20 rows");

    rdbms
        .execute(&key, &worker_id, &format!("DROP TABLE {table}"), vec![])
        .await
        .ok();
    let _ = rdbms.remove(&key, &worker_id).await;
}
