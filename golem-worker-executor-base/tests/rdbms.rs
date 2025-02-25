// Copyright 2024-2025 Golem Cloud
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

use std::collections::HashMap;
use std::fmt::Display;
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep};

use crate::common::{start, TestContext, TestWorkerExecutor};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;
use golem_common::model::{ComponentId, IdempotencyKey, OplogIndex, WorkerId, WorkerStatus};
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdbs;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use golem_test_framework::components::rdb::RdbsConnections;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::analysed_type;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{Value, ValueAndType};
use golem_worker_executor_base::services::rdbms::mysql::MysqlType;
use golem_worker_executor_base::services::rdbms::postgres::PostgresType;
use golem_worker_executor_base::services::rdbms::RdbmsType;
use serde_json::json;
use tokio::task::JoinSet;
use uuid::Uuid;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test_dep]
async fn postgres() -> DockerPostgresRdbs {
    DockerPostgresRdbs::new(3, 5424, true, false).await
}

#[test_dep]
async fn mysql() -> DockerMysqlRdbs {
    DockerMysqlRdbs::new(3, 3317, true, false).await
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
enum StatementAction {
    Execute,
    Query,
    QueryStream,
}

impl Display for StatementAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatementAction::Execute => write!(f, "execute"),
            StatementAction::Query => write!(f, "query"),
            StatementAction::QueryStream => write!(f, "query-stream"),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
enum TransactionEnd {
    Commit,
    Rollback,
    // no transaction end action, drop of transaction resource should do rollback
    None,
}

#[derive(Debug, Clone)]
struct StatementTest {
    pub action: StatementAction,
    pub statement: &'static str,
    pub params: Vec<String>,
    pub sleep: Option<u64>,
    pub expected: Option<serde_json::Value>,
}

impl StatementTest {
    fn execute_test(statement: &'static str, params: Vec<String>, expected: Option<u64>) -> Self {
        Self {
            action: StatementAction::Execute,
            statement,
            params,
            sleep: None,
            expected: expected.map(execute_ok_response),
        }
    }

    fn query_test(
        statement: &'static str,
        params: Vec<String>,
        expected: Option<serde_json::Value>,
    ) -> Self {
        Self {
            action: StatementAction::Query,
            statement,
            params,
            sleep: None,
            expected,
        }
    }

    fn query_stream_test(
        statement: &'static str,
        params: Vec<String>,
        expected: Option<serde_json::Value>,
    ) -> Self {
        Self {
            action: StatementAction::QueryStream,
            statement,
            params,
            sleep: None,
            expected,
        }
    }

    fn with_action(&self, action: StatementAction) -> Self {
        Self {
            action,
            ..self.clone()
        }
    }

    fn with_expected(&self, expected: Option<serde_json::Value>) -> Self {
        Self {
            expected,
            ..self.clone()
        }
    }

    // fn with_sleep(&self, sleep: u64) -> Self {
    //     Self {
    //         sleep: Some(sleep),
    //         ..self.clone()
    //     }
    // }
}

#[derive(Debug, Clone)]
struct RdbmsTest {
    statements: Vec<StatementTest>,
    transaction_end: Option<TransactionEnd>,
}

impl RdbmsTest {
    fn new(statements: Vec<StatementTest>, transaction_end: Option<TransactionEnd>) -> Self {
        Self {
            statements,
            transaction_end,
        }
    }

    fn fn_name(&self) -> &'static str {
        match self.transaction_end {
            Some(_) => "transaction",
            None => "executions",
        }
    }

    fn has_expected(&self) -> bool {
        for s in &self.statements {
            if s.expected.is_some() {
                return true;
            }
        }
        false
    }
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_crud(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdbs,
    _tracing: &Tracing,
) {
    let db_addresses: Vec<String> = postgres.host_connection_strings();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.component("rdbms-service").store().await;

    let worker_ids1 =
        start_workers::<PostgresType>(&executor, &component_id, db_addresses.clone(), 1).await;

    let worker_ids3 =
        start_workers::<PostgresType>(&executor, &component_id, db_addresses.clone(), 3).await;

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test_users
            (
                user_id             uuid    NOT NULL PRIMARY KEY,
                name                text    NOT NULL,
                tags                text[],
                created_on          timestamp DEFAULT NOW()
            );
        "#;

    let insert_statement = r#"
            INSERT INTO test_users
            (user_id, name, tags)
            VALUES
            ($1::uuid, $2, $3)
        "#;

    let count = 60;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        create_table_statement,
        vec![],
        None,
    ));

    let expected_values: Vec<(Uuid, String, String)> = postgres_get_values(count);

    for (user_id, name, tags) in expected_values.clone() {
        let params: Vec<String> = vec![user_id.to_string(), name, tags];

        insert_tests.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));
    }

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit)),
    )
    .await;

    let expected = postgres_get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_stream_test(
        "SELECT user_id, name, tags FROM test_users ORDER BY created_on ASC",
        vec![],
        Some(expected),
    );

    let expected = postgres_get_expected(vec![expected_values[0].clone()]);
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name, tags FROM test_users WHERE user_id = $1::uuid ORDER BY created_on ASC",
        vec![expected_values[0].0.to_string()],
        Some(expected),
    );

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(
            vec![select_test1.clone(), select_test2],
            Some(TransactionEnd::Commit),
        ),
    )
    .await;

    let delete = StatementTest::execute_test("DELETE FROM test_users", vec![], None);

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Rollback)),
    )
    .await;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::None)),
    )
    .await;

    let select_test = select_test1.with_action(StatementAction::Query);

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    let select_test = select_test.with_expected(Some(query_empty_ok_response()));

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    workers_resume_test(&executor, worker_ids1.clone()).await;

    workers_resume_test(&executor, worker_ids3.clone()).await;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    drop(executor);
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_idempotency(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdbs,
    _tracing: &Tracing,
) {
    let db_address = postgres.host_connection_strings()[0].clone();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.component("rdbms-service").store().await;

    let worker_ids =
        start_workers::<PostgresType>(&executor, &component_id, vec![db_address.clone()], 1).await;

    let worker_id = worker_ids[0].clone();

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test_users_idem
            (
                user_id             uuid    NOT NULL PRIMARY KEY,
                name                text    NOT NULL,
                tags                text[],
                created_on          timestamp DEFAULT NOW()
            );
        "#;

    let insert_statement = r#"
            INSERT INTO test_users_idem
            (user_id, name, tags)
            VALUES
            ($1::uuid, $2, $3)
        "#;

    let count = 10;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        create_table_statement,
        vec![],
        None,
    ));

    let expected_values: Vec<(Uuid, String, String)> = postgres_get_values(count);

    for (user_id, name, tags) in expected_values.clone() {
        let params: Vec<String> = vec![user_id.to_string(), name, tags];

        insert_tests.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));
    }

    let test = RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit));

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    let result2 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let expected = postgres_get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_stream_test(
        "SELECT user_id, name, tags FROM test_users_idem ORDER BY created_on ASC",
        vec![],
        Some(expected.clone()),
    );
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name, tags FROM test_users_idem ORDER BY created_on ASC",
        vec![],
        Some(expected.clone()),
    );

    let test = RdbmsTest::new(vec![select_test1.clone(), select_test2.clone()], None);

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    let result2 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let delete = StatementTest::execute_test("DELETE FROM test_users_idem", vec![], None);

    let test = RdbmsTest::new(
        vec![select_test1, select_test2, delete],
        Some(TransactionEnd::Commit),
    );

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    let result2 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;
    let oplog = serde_json::to_string(&oplog);
    check!(oplog.is_ok());
    // println!("worker {} oplog: {}", worker_id.clone(), oplog.unwrap());

    drop(executor);
}

// #[test]
// #[tracing::instrument]
// async fn rdbms_postgres_recovery(
//     last_unique_id: &LastUniqueId,
//     deps: &WorkerExecutorTestDependencies,
//     postgres: &DockerPostgresRdbs,
//     _tracing: &Tracing,
// ) {
//     let db_address = postgres.host_connection_strings()[0].clone();
//
//     let context = TestContext::new(last_unique_id);
//     let executor = start(deps, &context).await.unwrap();
//     let component_id = executor.component("rdbms-service").store().await;
//
//     let worker_ids =
//         start_workers::<PostgresType>(&executor, &component_id, vec![db_address.clone()], 2).await;
//
//     let worker_id = worker_ids[0].clone();
//     let worker_id2 = worker_ids[1].clone();
//
//     let create_table_statement = r#"
//             CREATE TABLE IF NOT EXISTS test_users_rec
//             (
//                 user_id             uuid    NOT NULL PRIMARY KEY,
//                 name                text    NOT NULL,
//                 tags                text[],
//                 created_on          timestamp DEFAULT NOW()
//             );
//         "#;
//
//     let insert_statement = r#"
//             INSERT INTO test_users_rec
//             (user_id, name, tags)
//             VALUES
//             ($1::uuid, $2, $3)
//         "#;
//
//     let test = RdbmsTest::new(
//         vec![StatementTest::execute_test(
//             create_table_statement,
//             vec![],
//             None,
//         )],
//         Some(TransactionEnd::Commit),
//     );
//
//     let result1 = execute_worker_test::<PostgresType>(
//         &executor,
//         &worker_id,
//         &IdempotencyKey::fresh(),
//         test.clone(),
//     )
//     .await;
//
//     check_test_result(&worker_id, result1.clone(), test.clone());
//
//     let count = 10;
//
//     let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count);
//
//     let expected_values: Vec<(Uuid, String, String)> = postgres_get_values(count);
//
//     for (index, (user_id, name, tags)) in expected_values.iter().enumerate() {
//         let params: Vec<String> = vec![user_id.to_string(), name.clone(), tags.clone()];
//         if index == (count / 2) {
//             insert_tests.push(StatementTest::execute_test(
//                 insert_statement,
//                 params.clone(),
//                 Some(1),
//             ).with_sleep(3000));
//         } else {
//             insert_tests.push(StatementTest::execute_test(
//                 insert_statement,
//                 params.clone(),
//                 Some(1),
//             ));
//         }
//     }
//
//     let mut worker_output = executor.capture_output(&worker_id).await;
//     let test = RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit));
//
//     let executor_clone = executor.clone();
//     let worker_id_clone = worker_id.clone();
//     let test_clone = test.clone();
//     let _fiber = spawn(async move {
//         let _ = execute_worker_test::<PostgresType>(
//             &executor_clone,
//             &worker_id_clone,
//             &IdempotencyKey::fresh(),
//             test_clone,
//         )
//         .await;
//     });
//
//     tokio::time::sleep(Duration::from_millis(1000)).await;
//     let _ = executor.interrupt(&worker_id).await;
//     // let _ = fiber.await.unwrap();
//
//     // drop(executor);
//     //
//     // let executor = start(deps, &context).await.unwrap();
//
//     let select_test1 = StatementTest::query_stream_test(
//         "SELECT user_id, name, tags FROM test_users_rec ORDER BY created_on ASC",
//         vec![],
//         Some(postgres_get_expected(vec![])),
//     );
//
//     let test = RdbmsTest::new(vec![select_test1.clone()], Some(TransactionEnd::Commit));
//
//     let result2 = execute_worker_test::<PostgresType>(
//         &executor,
//         &worker_id2,
//         &IdempotencyKey::fresh(),
//         test.clone(),
//     )
//     .await;
//
//     check_test_result(&worker_id, result2.clone(), test.clone());
//
//
//     let mut events = vec![];
//     worker_output.recv_many(&mut events, 100).await;
//
//     println!("worker {} events: {:?}", worker_id.clone(), events);
//
//     let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;
//     let oplog = serde_json::to_string(&oplog).unwrap();
//     println!("worker {} oplog: {}", worker_id.clone(), oplog);
//
//     let metadata = executor.get_worker_metadata(&worker_id).await;
//     println!("worker {} metadata: {:?}", worker_id.clone(), metadata);
//
//     executor.resume(&worker_id, false).await;
//
//     let _metadata = executor
//         .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
//         .await;
//
//     let expected = postgres_get_expected(expected_values.clone());
//
//     let test = RdbmsTest::new(
//         vec![select_test1.clone().with_expected(Some(expected))],
//         Some(TransactionEnd::Commit),
//     );
//
//     let result1 = execute_worker_test::<PostgresType>(
//         &executor,
//         &worker_id,
//         &IdempotencyKey::fresh(),
//         test.clone(),
//     )
//     .await;
//
//     check_test_result(&worker_id, result1.clone(), test.clone());
//
//     drop(executor);
// }

fn postgres_get_values(count: usize) -> Vec<(Uuid, String, String)> {
    let mut values: Vec<(Uuid, String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = Uuid::new_v4();
        let name = format!("name-{}", Uuid::new_v4());
        let vs: Vec<String> = (0..5).map(|v| format!("tag-{}-{}", v, i)).collect();
        let tags = format!("[{}]", vs.join(", "));

        values.push((user_id, name, tags));
    }
    values
}

fn postgres_get_row(columns: (Uuid, String, String)) -> serde_json::Value {
    let user_id = columns.0.as_u64_pair();
    json!(
        {
           "values":[
              {
                    "uuid":  {
                       "high-bits": user_id.0,
                       "low-bits": user_id.1
                    }

              },
              {
                    "text": columns.1
              },
              {

                    "text": columns.2
              }
           ]
        }
    )
}

fn postgres_get_expected(expected_values: Vec<(Uuid, String, String)>) -> serde_json::Value {
    let expected_rows: Vec<serde_json::Value> =
        expected_values.into_iter().map(postgres_get_row).collect();

    let expected_columns: Vec<serde_json::Value> = if expected_rows.is_empty() {
        vec![]
    } else {
        vec![
            json!({
               "db-type":{
                  "uuid":null
               },
               "db-type-name":"UUID",
               "name":"user_id",
               "ordinal":0
            }),
            json!({
               "db-type":{
                  "text":null
               },
               "db-type-name":"TEXT",
               "name":"name",
               "ordinal":1
            }),
            json!({
               "db-type":{
                     "text":null
               },
               "db-type-name":"TEXT[]",
               "name":"tags",
               "ordinal":2
            }),
        ]
    };
    query_ok_response(expected_columns, expected_rows)
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdbs,
    _tracing: &Tracing,
) {
    let test1 = StatementTest::execute_test("SELECT 1", vec![], Some(1));

    let expected_rows: Vec<serde_json::Value> = vec![json!({
       "values":[
          {
             "int4":1
          }
       ]
    })];
    let expected_columns: Vec<serde_json::Value> = vec![json!({
        "db-type":{
              "int4":null
        },
        "db-type-name":"INT4",
        "name":"?column?",
        "ordinal":0
    })];
    let expected = query_ok_response(expected_columns, expected_rows);

    let test2 = StatementTest::query_test("SELECT 1", vec![], Some(expected));

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        postgres.host_connection_strings(),
        RdbmsTest::new(vec![test1, test2], None),
        3,
    )
    .await;
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_crud(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdbs,
    _tracing: &Tracing,
) {
    let db_addresses: Vec<String> = mysql.host_connection_strings();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.component("rdbms-service").store().await;

    let worker_ids1 =
        start_workers::<MysqlType>(&executor, &component_id, db_addresses.clone(), 1).await;

    let worker_ids3 =
        start_workers::<MysqlType>(&executor, &component_id, db_addresses.clone(), 3).await;

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test_users
            (
                user_id             varchar(25)    NOT NULL,
                name                varchar(255)    NOT NULL,
                created_on          timestamp NOT NULL DEFAULT NOW(),
                PRIMARY KEY (user_id)
            );
        "#;

    let insert_statement = r#"
            INSERT INTO test_users
            (user_id, name)
            VALUES
            (?, ?)
        "#;

    let count = 60;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        create_table_statement,
        vec![],
        None,
    ));

    let expected_values: Vec<(String, String)> = mysql_get_values(count);

    for (user_id, name) in expected_values.clone() {
        let params: Vec<String> = vec![user_id, name];

        insert_tests.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));
    }

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit)),
    )
    .await;

    let expected = mysql_get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_stream_test(
        "SELECT user_id, name FROM test_users ORDER BY user_id ASC",
        vec![],
        Some(expected),
    );

    let expected = mysql_get_expected(vec![expected_values[0].clone()]);
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name FROM test_users WHERE user_id = ? ORDER BY user_id ASC",
        vec![expected_values[0].clone().0],
        Some(expected),
    );

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(
            vec![select_test1.clone(), select_test2],
            Some(TransactionEnd::Commit),
        ),
    )
    .await;

    let delete = StatementTest::execute_test("DELETE FROM test_users", vec![], None);

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Rollback)),
    )
    .await;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::None)),
    )
    .await;

    let select_test = select_test1.with_action(StatementAction::Query);

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    let select_test = select_test.with_expected(Some(query_empty_ok_response()));

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    workers_resume_test(&executor, worker_ids1.clone()).await;

    workers_resume_test(&executor, worker_ids3.clone()).await;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await;

    drop(executor);
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_idempotency(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdbs,
    _tracing: &Tracing,
) {
    let db_address = mysql.host_connection_strings()[0].clone();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.component("rdbms-service").store().await;

    let worker_ids =
        start_workers::<MysqlType>(&executor, &component_id, vec![db_address.clone()], 1).await;

    let worker_id = worker_ids[0].clone();

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test_users_idem
            (
                user_id             varchar(25)    NOT NULL,
                name                varchar(255)    NOT NULL,
                created_on          timestamp NOT NULL DEFAULT NOW(),
                PRIMARY KEY (user_id)
            );
        "#;

    let insert_statement = r#"
            INSERT INTO test_users_idem
            (user_id, name)
            VALUES
            (?, ?)
        "#;

    let count = 10;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        create_table_statement,
        vec![],
        None,
    ));

    let expected_values: Vec<(String, String)> = mysql_get_values(count);

    for (user_id, name) in expected_values.clone() {
        let params: Vec<String> = vec![user_id, name];

        insert_tests.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));
    }

    let test = RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit));

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    let result2 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let expected = mysql_get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_stream_test(
        "SELECT user_id, name FROM test_users_idem ORDER BY user_id ASC",
        vec![],
        Some(expected.clone()),
    );
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name FROM test_users_idem ORDER BY user_id ASC",
        vec![],
        Some(expected.clone()),
    );

    let test = RdbmsTest::new(vec![select_test1.clone(), select_test2.clone()], None);

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    let result2 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;
    check!(result2 == result1);

    let delete = StatementTest::execute_test("DELETE FROM test_users_idem", vec![], None);

    let test = RdbmsTest::new(
        vec![select_test1, select_test2, delete],
        Some(TransactionEnd::Commit),
    );

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    let result2 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;
    let oplog = serde_json::to_string(&oplog);
    check!(oplog.is_ok());
    // println!("worker {} oplog: {}", worker_id.clone(), oplog.unwrap());

    drop(executor);
}

fn mysql_get_values(count: usize) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = format!("{:03}", i);
        let name = format!("name-{}", Uuid::new_v4());

        values.push((user_id, name));
    }
    values
}

fn mysql_get_row(columns: (String, String)) -> serde_json::Value {
    json!(
        {
            "values":[
              {
                  "varchar": columns.0

              },
              {

                  "varchar": columns.1
              }
           ]
        }
    )
}

fn mysql_get_expected(expected_values: Vec<(String, String)>) -> serde_json::Value {
    let expected_rows: Vec<serde_json::Value> =
        expected_values.into_iter().map(mysql_get_row).collect();

    let expected_columns: Vec<serde_json::Value> = if expected_rows.is_empty() {
        vec![]
    } else {
        vec![
            json!(
            {
               "db-type":{
                  "varchar":null
               },
               "db-type-name":"VARCHAR",
               "name":"user_id",
               "ordinal":0
            }),
            json!(
            {
               "db-type":{
                  "varchar":null
               },
               "db-type-name":"VARCHAR",
               "name":"name",
               "ordinal":1
            }),
        ]
    };
    query_ok_response(expected_columns, expected_rows)
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdbs,
    _tracing: &Tracing,
) {
    let test1 = StatementTest::execute_test("SELECT 1", vec![], Some(0));

    let expected_rows: Vec<serde_json::Value> = vec![json!({
       "values":[
          {
             "bigint":1
          }
       ]
    })];
    let expected_columns: Vec<serde_json::Value> = vec![json!({
        "db-type":{
           "bigint":null
        },
        "db-type-name":"BIGINT",
        "name":"1",
        "ordinal":0
    })];
    let expected = query_ok_response(expected_columns, expected_rows);

    let test2 = StatementTest::query_test("SELECT 1", vec![], Some(expected));

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        mysql.host_connection_strings(),
        RdbmsTest::new(vec![test1, test2], None),
        1,
    )
    .await;
}

async fn rdbms_component_test<T: RdbmsType>(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_addresses: Vec<String>,
    test: RdbmsTest,
    workers_per_db: u8,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.component("rdbms-service").store().await;
    let worker_ids =
        start_workers::<T>(&executor, &component_id, db_addresses, workers_per_db).await;

    rdbms_workers_test::<T>(&executor, worker_ids, test).await;

    drop(executor);
}

async fn rdbms_workers_test<T: RdbmsType>(
    executor: &TestWorkerExecutor,
    worker_ids: Vec<WorkerId>,
    test: RdbmsTest,
) {
    let mut workers_results: HashMap<WorkerId, Result<TypeAnnotatedValue, Error>> = HashMap::new(); // <worker_id, results>

    let mut fibers = JoinSet::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let executor_clone = executor.clone();
        let test_clone = test.clone();
        let _ = fibers.spawn(async move {
            let result = execute_worker_test::<T>(
                &executor_clone,
                &worker_id_clone,
                &IdempotencyKey::fresh(),
                test_clone,
            )
            .await;
            (worker_id_clone, result)
        });
    }

    while let Some(res) = fibers.join_next().await {
        let (worker_id, result_execute) = res.unwrap();
        workers_results.insert(worker_id, result_execute);
    }

    for (worker_id, result) in workers_results {
        check_test_result(&worker_id, result, test.clone());
    }
}

async fn execute_worker_test<T: RdbmsType>(
    executor: &TestWorkerExecutor,
    worker_id: &WorkerId,
    idempotency_key: &IdempotencyKey,
    test: RdbmsTest,
) -> Result<TypeAnnotatedValue, Error> {
    let db_type = T::default().to_string();

    let fn_name = test.fn_name();
    let component_fn_name = format!("golem:it/api.{{{db_type}-{fn_name}}}");

    let mut statements: Vec<Value> = Vec::with_capacity(test.statements.len());

    for s in test.statements {
        let params = Value::List(s.params.into_iter().map(Value::String).collect());
        statements.push(Value::Record(vec![
            Value::String(s.statement.to_string()),
            params,
            Value::Enum(s.action as u32),
            Value::Option(s.sleep.map(|v| Box::new(Value::U64(v)))),
        ]));
    }

    let statements = ValueAndType::new(
        Value::List(statements),
        analysed_type::list(analysed_type::record(vec![
            analysed_type::field("statement", analysed_type::str()),
            analysed_type::field("params", analysed_type::list(analysed_type::str())),
            analysed_type::field(
                "action",
                analysed_type::r#enum(&["execute", "query", "query-stream"]),
            ),
            analysed_type::field("sleep", analysed_type::option(analysed_type::u64())),
        ])),
    );

    let mut fn_params: Vec<ValueAndType> = vec![statements];

    if let Some(te) = test.transaction_end {
        fn_params.push(ValueAndType::new(
            Value::Enum(te as u32),
            analysed_type::r#enum(&["commit", "rollback", "none"]),
        ));
    }

    executor
        .invoke_and_await_typed_with_key(
            worker_id,
            idempotency_key,
            component_fn_name.as_str(),
            fn_params,
        )
        .await
}

fn check_test_result(
    worker_id: &WorkerId,
    result: Result<TypeAnnotatedValue, Error>,
    test: RdbmsTest,
) {
    let fn_name = test.fn_name();

    check!(
        result.is_ok(),
        "result {fn_name} for worker {worker_id} is ok"
    );

    let response = result.clone().unwrap().to_json_value();

    let response = response
        .as_array()
        .and_then(|v| v.first())
        .and_then(|v| v.as_object())
        .cloned();

    if test.has_expected() {
        let ok_response = response
            .and_then(|v| v.get("ok").cloned())
            .and_then(|v| v.as_array().cloned());

        if let Some(response_values) = ok_response {
            for (index, test_statement) in test.statements.into_iter().enumerate() {
                let action = test_statement.action;
                if let Some(expected) = test_statement.expected {
                    match response_values.get(index).cloned() {
                        Some(response) => {
                            // println!("{}", response);
                            check!(
                                    response == expected,
                                    "result {fn_name} {action} with index {index} for worker {worker_id} match"
                                );
                        }
                        None => {
                            check!(
                                    false,
                                    "result {fn_name} {action} with index {index} for worker {worker_id} is not found"
                                );
                        }
                    }
                }
            }
        } else {
            check!(false, "result {fn_name} for worker {worker_id} is not ok");
        }
    }
}

async fn start_workers<T: RdbmsType>(
    executor: &TestWorkerExecutor,
    component_id: &ComponentId,
    db_addresses: Vec<String>,
    workers_per_db: u8,
) -> Vec<WorkerId> {
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let db_type = T::default().to_string();
    let db_env_var = format!("DB_{}_URL", db_type.to_uppercase());

    for db_address in db_addresses.into_iter() {
        let mut env = HashMap::new();
        env.insert(db_env_var.clone(), db_address);
        for _ in 0..workers_per_db {
            let worker_name = format!("rdbms-service-{}-{}", db_type, Uuid::new_v4());
            let worker_id = executor
                .start_worker_with(component_id, &worker_name, vec![], env.clone())
                .await;
            worker_ids.push(worker_id.clone());
            let _result = executor
                .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
                .await
                .unwrap();
        }
    }
    worker_ids
}

async fn workers_resume_test(executor: &TestWorkerExecutor, worker_ids: Vec<WorkerId>) {
    let mut workers_results: HashMap<WorkerId, WorkerStatus> = HashMap::new();

    let mut fibers = JoinSet::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let executor_clone = executor.clone();
        let _ = fibers.spawn(async move {
            executor_clone.interrupt(&worker_id_clone).await;
            executor_clone.resume(&worker_id_clone, false).await;
            let metadata = executor_clone
                .wait_for_status(&worker_id_clone, WorkerStatus::Idle, Duration::from_secs(3))
                .await;
            let status = metadata.last_known_status.status;
            (worker_id_clone, status)
        });
    }

    while let Some(res) = fibers.join_next().await {
        let (worker_id, status) = res.unwrap();
        workers_results.insert(worker_id, status);
    }

    for (worker_id, status) in workers_results {
        check!(
            status == WorkerStatus::Idle,
            "status for worker {worker_id} is Idle"
        );
    }
}

fn execute_ok_response(value: u64) -> serde_json::Value {
    json!(
       {
          "ok": {
              "execute": value
           }
       }
    )
}

fn query_ok_response(
    columns: Vec<serde_json::Value>,
    rows: Vec<serde_json::Value>,
) -> serde_json::Value {
    json!(
        {
          "ok":{
             "query":{
                 "columns": columns,
                 "rows": rows
             }
           }
       }
    )
}

fn query_empty_ok_response() -> serde_json::Value {
    query_ok_response(vec![], vec![])
}
