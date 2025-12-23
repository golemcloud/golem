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

use crate::Tracing;
use assert2::check;
use golem_common::model::component::ComponentId;
use golem_common::model::{IdempotencyKey, OplogIndex, WorkerId, WorkerStatus};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdb;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::analysis::analysed_type;
use golem_wasm::{Value, ValueAndType};
use golem_worker_executor::services::rdbms::mysql::MysqlType;
use golem_worker_executor::services::rdbms::postgres::PostgresType;
use golem_worker_executor::services::rdbms::RdbmsType;
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, TestWorkerExecutor, WorkerExecutorTestDependencies,
};
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Display;
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep};
use tokio::task::JoinSet;
use tracing::Instrument;
use uuid::Uuid;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test_dep]
async fn postgres() -> DockerPostgresRdb {
    let unique_network_id = Uuid::new_v4().to_string();
    DockerPostgresRdb::new(&unique_network_id, false).await
}

#[test_dep]
async fn mysql() -> DockerMysqlRdb {
    let unique_network_id = Uuid::new_v4().to_string();
    DockerMysqlRdb::new(&unique_network_id).await
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
    pub statement: String,
    pub params: Vec<String>,
    pub sleep: Option<u64>,
    pub expected: Option<serde_json::Value>,
}

impl StatementTest {
    fn execute_test(statement: String, params: Vec<String>, expected: Option<u64>) -> Self {
        Self {
            action: StatementAction::Execute,
            statement,
            params,
            sleep: None,
            expected: expected.map(execute_response),
        }
    }

    fn query_test(
        statement: String,
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
        statement: String,
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

#[derive(Clone, Eq, PartialEq, Debug)]
enum TransactionFailOn {
    OplogAdd(String, u8),
    OplogAddAndTx(String, u8, String, u8),
}

impl TransactionFailOn {
    fn oplog_add(entry: &str, fail_count: u8) -> Self {
        Self::OplogAdd(entry.to_string(), fail_count)
    }

    fn oplog_add_and_tx(
        oplog_add_entry: &str,
        oplog_add_fail_count: u8,
        tx_entry: &str,
        tx_fail_count: u8,
    ) -> Self {
        Self::OplogAddAndTx(
            oplog_add_entry.to_string(),
            oplog_add_fail_count,
            tx_entry.to_string(),
            tx_fail_count,
        )
    }

    fn name(&self) -> String {
        match self {
            TransactionFailOn::OplogAdd(e, c) => format!("FailOplogAdd{c}On{e}"),
            TransactionFailOn::OplogAddAndTx(oe, oc, te, tc) => {
                format!("FailOplogAdd{oc}On{oe}-FailRdbmsTx{tc}On{te}")
            }
        }
    }
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_crud(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let db_address = postgres.public_connection_string();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids_1 =
        start_workers::<PostgresType>(&executor, &component.id, &db_address, "", 1).await?;

    let worker_ids3 =
        start_workers::<PostgresType>(&executor, &component.id, &db_address, "", 3).await?;

    let table_name = "test_users";

    let count = 60;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        postgres_create_table_statement(table_name),
        vec![],
        None,
    ));

    let expected_values: Vec<(Uuid, String, String)> = postgres_get_values(count);
    insert_tests.append(&mut postgres_insert_statements(
        table_name,
        expected_values.clone(),
        Some(1),
    ));

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids_1.clone(),
        RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit)),
    )
    .await?;

    let expected = postgres_get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_stream_test(
        postgres_select_statement(table_name),
        vec![],
        Some(expected),
    );

    let expected = postgres_get_expected(vec![expected_values[0].clone()]);
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name, tags FROM test_users WHERE user_id = $1::uuid ORDER BY created_on ASC".to_string(),
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
    .await?;

    let delete = StatementTest::execute_test("DELETE FROM test_users".to_string(), vec![], None);

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids_1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Rollback)),
    )
    .await?;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids_1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::None)),
    )
    .await?;

    let select_test = select_test1.with_action(StatementAction::Query);

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids_1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    let select_test = select_test.with_expected(Some(query_empty_response()));

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids_1.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    let worker_id = worker_ids_1[0].clone();
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    workers_interrupt_test(&executor, worker_ids_1.clone()).await?;
    workers_interrupt_test(&executor, worker_ids3.clone()).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids_1.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    rdbms_workers_test::<PostgresType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_idempotency(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let db_address = postgres.public_connection_string();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids =
        start_workers::<PostgresType>(&executor, &component.id, &db_address, "", 1).await?;

    let worker_id = worker_ids[0].clone();

    let table_name = "test_users_idem";

    let count = 10;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        postgres_create_table_statement(table_name),
        vec![],
        None,
    ));

    let expected_values: Vec<(Uuid, String, String)> = postgres_get_values(count);
    insert_tests.append(&mut postgres_insert_statements(
        table_name,
        expected_values.clone(),
        Some(1),
    ));

    let test = RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit));

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    let result2 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let select_tests = postgres_select_statements(table_name, expected_values);

    let test = RdbmsTest::new(select_tests.clone(), None);

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    let result2 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let delete =
        StatementTest::execute_test("DELETE FROM test_users_idem".to_string(), vec![], None);

    let test = RdbmsTest::new(
        [select_tests, vec![delete]].concat(),
        Some(TransactionEnd::Commit),
    );

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    let result2 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    drop(executor);
    let executor = start(deps, &context).await?;

    let select_tests = postgres_select_statements(table_name, vec![]);
    let test = RdbmsTest::new(select_tests.clone(), None);

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<PostgresType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    Ok(())
}

async fn postgres_transaction_recovery_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    fail_on: TransactionFailOn,
    transaction_end: TransactionEnd,
) -> anyhow::Result<()> {
    let db_address = postgres.public_connection_string();
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids = start_workers::<PostgresType>(
        &executor,
        &component.id,
        &db_address,
        format!("-{}", fail_on.name()).as_str(),
        1,
    )
    .await?;

    let worker_id = worker_ids[0].clone();

    let table_name = format!(
        "test_users_{}",
        Uuid::new_v4().to_string().replace("-", "_")
    );

    let create_test = RdbmsTest::new(
        vec![StatementTest::execute_test(
            postgres_create_table_statement(&table_name),
            vec![],
            None,
        )],
        None,
    );

    let result1 = execute_worker_test::<PostgresType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        create_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), create_test.clone());

    let count = 3;

    let expected_values: Vec<(Uuid, String, String)> = postgres_get_values(count);

    let insert_test = RdbmsTest::new(
        postgres_insert_statements(&table_name, expected_values.clone(), None),
        Some(transaction_end),
    );

    let result1 = execute_worker_test::<PostgresType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        insert_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), insert_test.clone());

    let select_test = if transaction_end == TransactionEnd::Commit {
        RdbmsTest::new(
            postgres_select_statements(&table_name, expected_values),
            None,
        )
    } else {
        RdbmsTest::new(postgres_select_statements(&table_name, vec![]), None)
    };

    let result1 = execute_worker_test::<PostgresType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        select_test.clone(),
    )
    .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check_test_result(&worker_id, result1.clone(), select_test.clone());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    workers_interrupt_test(&executor, worker_ids.clone()).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result1 = execute_worker_test::<PostgresType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        select_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), select_test.clone());

    let delete = StatementTest::execute_test(format!("DELETE FROM {table_name}"), vec![], None);

    let test = RdbmsTest::new(vec![delete], None);

    let result1 = execute_worker_test::<PostgresType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_commit_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for fail_count in 1..=2 {
        postgres_transaction_recovery_test(
            last_unique_id,
            deps,
            postgres,
            TransactionFailOn::oplog_add("CommittedRemoteTransaction", fail_count),
            TransactionEnd::Commit,
        )
        .await?;
    }
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_pre_commit_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    postgres_transaction_recovery_test(
        last_unique_id,
        deps,
        postgres,
        TransactionFailOn::oplog_add("PreCommitRemoteTransaction", 1),
        TransactionEnd::Commit,
    )
    .await?;

    postgres_transaction_recovery_test(
        last_unique_id,
        deps,
        postgres,
        TransactionFailOn::oplog_add("PreCommitRemoteTransaction", 2),
        TransactionEnd::Commit,
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_rollback_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for fail_count in 1..=2 {
        postgres_transaction_recovery_test(
            last_unique_id,
            deps,
            postgres,
            TransactionFailOn::oplog_add("RolledBackRemoteTransaction", fail_count),
            TransactionEnd::Rollback,
        )
        .await?;
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_pre_rollback_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    postgres_transaction_recovery_test(
        last_unique_id,
        deps,
        postgres,
        TransactionFailOn::oplog_add("PreRollbackRemoteTransaction", 1),
        TransactionEnd::Rollback,
    )
    .await?;

    postgres_transaction_recovery_test(
        last_unique_id,
        deps,
        postgres,
        TransactionFailOn::oplog_add("PreRollbackRemoteTransaction", 2),
        TransactionEnd::Rollback,
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[ignore] // This test simulates that the commit succeeds, but we neither write this to the oplog or can query this fact from the DB. In this case the transaction gets retried and it leads to violations, which is expected, but the current test does not expect it.
async fn rdbms_postgres_commit_and_tx_status_not_found_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    postgres_transaction_recovery_test(
        last_unique_id,
        deps,
        postgres,
        TransactionFailOn::oplog_add_and_tx(
            "CommittedRemoteTransaction",
            1,
            "GetTransactionStatusNotFound",
            1,
        ),
        TransactionEnd::Commit,
    )
    .await?;

    Ok(())
}

fn postgres_create_table_statement(table_name: &str) -> String {
    format!(
        r#"
            CREATE TABLE IF NOT EXISTS {table_name}
            (
                user_id             uuid    NOT NULL PRIMARY KEY,
                name                text    NOT NULL,
                tags                text[],
                created_on          timestamp DEFAULT NOW()
            );
        "#
    )
}

fn postgres_insert_statement(table_name: &str) -> String {
    format!(
        r#"
         INSERT INTO {table_name}
            (user_id, name, tags)
            VALUES
            ($1::uuid, $2, $3)
        "#
    )
}

fn postgres_select_statement(table_name: &str) -> String {
    format!("SELECT user_id, name, tags FROM {table_name} ORDER BY created_on ASC")
}

fn postgres_get_values(count: usize) -> Vec<(Uuid, String, String)> {
    let mut values: Vec<(Uuid, String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = Uuid::new_v4();
        let name = format!("name-{}", Uuid::new_v4());
        let vs: Vec<String> = (0..5).map(|v| format!("tag-{v}-{i}")).collect();
        let tags = format!("[{}]", vs.join(", "));

        values.push((user_id, name, tags));
    }
    values
}

fn postgres_insert_statements(
    table_name: &str,
    values: Vec<(Uuid, String, String)>,
    expected: Option<u64>,
) -> Vec<StatementTest> {
    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(values.len());
    for (user_id, name, tags) in values {
        let params: Vec<String> = vec![user_id.to_string(), name, tags];

        insert_tests.push(StatementTest::execute_test(
            postgres_insert_statement(table_name),
            params.clone(),
            expected,
        ));
    }
    insert_tests
}

fn postgres_select_statements(
    table_name: &str,
    expected_values: Vec<(Uuid, String, String)>,
) -> Vec<StatementTest> {
    let expected = postgres_get_expected(expected_values);
    let select_test1 = StatementTest::query_stream_test(
        postgres_select_statement(table_name),
        vec![],
        Some(expected.clone()),
    );
    let select_test2 = StatementTest::query_test(
        postgres_select_statement(table_name),
        vec![],
        Some(expected.clone()),
    );

    vec![select_test1.clone(), select_test2.clone()]
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
    query_response(expected_columns, expected_rows)
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let test1 = StatementTest::execute_test("SELECT 1".to_string(), vec![], Some(1));

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
    let expected = query_response(expected_columns, expected_rows);

    let test2 = StatementTest::query_test("SELECT 1".to_string(), vec![], Some(expected));

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        &postgres.public_connection_string(),
        RdbmsTest::new(vec![test1, test2], None),
        3,
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_crud(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let db_address = mysql.public_connection_string();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids1 =
        start_workers::<MysqlType>(&executor, &component.id, &db_address, "", 1).await?;

    let worker_ids3 =
        start_workers::<MysqlType>(&executor, &component.id, &db_address, "", 3).await?;

    let table_name = "test_users";

    let count = 60;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        mysql_create_table_statement(table_name),
        vec![],
        None,
    ));

    let expected_values: Vec<(String, String)> = mysql_get_values(count);

    insert_tests.append(&mut mysql_insert_statements(
        table_name,
        expected_values.clone(),
        Some(1),
    ));

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit)),
    )
    .await?;

    let expected = mysql_get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_stream_test(
        mysql_select_statement(table_name),
        vec![],
        Some(expected),
    );

    let expected = mysql_get_expected(vec![expected_values[0].clone()]);
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name FROM test_users WHERE user_id = ? ORDER BY user_id ASC".to_string(),
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
    .await?;

    let delete = StatementTest::execute_test("DELETE FROM test_users".to_string(), vec![], None);

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Rollback)),
    )
    .await?;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::None)),
    )
    .await?;

    let select_test = select_test1.with_action(StatementAction::Query);

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    let select_test = select_test.with_expected(Some(query_empty_response()));

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    let worker_id = worker_ids1[0].clone();
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    workers_interrupt_test(&executor, worker_ids1.clone()).await?;
    workers_interrupt_test(&executor, worker_ids3.clone()).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids1.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    rdbms_workers_test::<MysqlType>(
        &executor,
        worker_ids3.clone(),
        RdbmsTest::new(vec![select_test.clone()], Some(TransactionEnd::Commit)),
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_idempotency(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let db_address = mysql.public_connection_string();

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids =
        start_workers::<MysqlType>(&executor, &component.id, &db_address, "", 1).await?;

    let worker_id = worker_ids[0].clone();

    let table_name = "test_users_idem";

    let count = 10;

    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(count + 1);

    insert_tests.push(StatementTest::execute_test(
        mysql_create_table_statement(table_name),
        vec![],
        None,
    ));

    let expected_values: Vec<(String, String)> = mysql_get_values(count);

    insert_tests.append(&mut mysql_insert_statements(
        table_name,
        expected_values.clone(),
        Some(1),
    ));

    let test = RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit));

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    let result2 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    let select_tests = mysql_select_statements(table_name, expected_values);
    let test = RdbmsTest::new(select_tests.clone(), None);

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    let result2 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;
    check!(result2 == result1);

    let delete =
        StatementTest::execute_test("DELETE FROM test_users_idem".to_string(), vec![], None);

    let test = RdbmsTest::new(
        [select_tests, vec![delete]].concat(),
        Some(TransactionEnd::Commit),
    );

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    let result2 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    check!(result2 == result1);

    drop(executor);
    let executor = start(deps, &context).await?;

    let select_tests = mysql_select_statements(table_name, vec![]);
    let test = RdbmsTest::new(select_tests.clone(), None);

    let idempotency_key = IdempotencyKey::fresh();

    let result1 =
        execute_worker_test::<MysqlType>(&executor, &worker_id, &idempotency_key, test.clone())
            .await?;

    check_test_result(&worker_id, result1, test.clone());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    Ok(())
}

async fn mysql_transaction_recovery_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    fail_on: TransactionFailOn,
    transaction_end: TransactionEnd,
) -> anyhow::Result<()> {
    let db_address = mysql.public_connection_string();
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids = start_workers::<MysqlType>(
        &executor,
        &component.id,
        &db_address,
        format!("-{}", fail_on.name()).as_str(),
        1,
    )
    .await?;

    let worker_id = worker_ids[0].clone();

    let table_name = format!(
        "test_users_{}",
        Uuid::new_v4().to_string().replace("-", "_")
    );

    let create_test = RdbmsTest::new(
        vec![StatementTest::execute_test(
            mysql_create_table_statement(&table_name),
            vec![],
            None,
        )],
        None,
    );

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        create_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), create_test.clone());

    let count = 3;

    let expected_values: Vec<(String, String)> = mysql_get_values(count);

    let insert_test = RdbmsTest::new(
        mysql_insert_statements(&table_name, expected_values.clone(), None),
        Some(transaction_end),
    );

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        insert_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), insert_test.clone());

    let select_test = if transaction_end == TransactionEnd::Commit {
        RdbmsTest::new(mysql_select_statements(&table_name, expected_values), None)
    } else {
        RdbmsTest::new(mysql_select_statements(&table_name, vec![]), None)
    };

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        select_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), select_test.clone());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    workers_interrupt_test(&executor, worker_ids.clone()).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        select_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), select_test.clone());

    let delete = StatementTest::execute_test(format!("DELETE FROM {table_name}"), vec![], None);

    let test = RdbmsTest::new(vec![delete], None);

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), test.clone());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let oplog_json = serde_json::to_string(&oplog);
    check!(oplog_json.is_ok());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_commit_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for fail_count in 1..=2 {
        mysql_transaction_recovery_test(
            last_unique_id,
            deps,
            mysql,
            TransactionFailOn::oplog_add("CommittedRemoteTransaction", fail_count),
            TransactionEnd::Commit,
        )
        .await?;
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_pre_commit_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    mysql_transaction_recovery_test(
        last_unique_id,
        deps,
        mysql,
        TransactionFailOn::oplog_add("PreCommitRemoteTransaction", 1),
        TransactionEnd::Commit,
    )
    .await?;

    mysql_transaction_recovery_test(
        last_unique_id,
        deps,
        mysql,
        TransactionFailOn::oplog_add("PreCommitRemoteTransaction", 2),
        TransactionEnd::Commit,
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_rollback_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for fail_count in 1..=2 {
        mysql_transaction_recovery_test(
            last_unique_id,
            deps,
            mysql,
            TransactionFailOn::oplog_add("RolledBackRemoteTransaction", fail_count),
            TransactionEnd::Rollback,
        )
        .await?;
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_pre_rollback_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    mysql_transaction_recovery_test(
        last_unique_id,
        deps,
        mysql,
        TransactionFailOn::oplog_add("PreRollbackRemoteTransaction", 1),
        TransactionEnd::Rollback,
    )
    .await?;

    mysql_transaction_recovery_test(
        last_unique_id,
        deps,
        mysql,
        TransactionFailOn::oplog_add("PreRollbackRemoteTransaction", 2),
        TransactionEnd::Rollback,
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[ignore] // This test simulates that the commit succeeds, but we neither write this to the oplog or can query this fact from the DB. In this case the transaction gets retried and it leads to violations, which is expected, but the current test does not expect it.
async fn rdbms_mysql_commit_and_tx_status_not_found_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    mysql_transaction_recovery_test(
        last_unique_id,
        deps,
        mysql,
        TransactionFailOn::oplog_add_and_tx(
            "CommittedRemoteTransaction",
            1,
            "GetTransactionStatusNotFound",
            1,
        ),
        TransactionEnd::Commit,
    )
    .await?;
    Ok(())
}

fn mysql_create_table_statement(table_name: &str) -> String {
    format!(
        r#"
           CREATE TABLE IF NOT EXISTS {table_name}
             (
                user_id             varchar(25)    NOT NULL,
                name                varchar(255)    NOT NULL,
                created_on          timestamp NOT NULL DEFAULT NOW(),
                PRIMARY KEY (user_id)
            );
        "#
    )
}

fn mysql_insert_statement(table_name: &str) -> String {
    format!(
        r#"
            INSERT INTO {table_name}
            (user_id, name)
            VALUES
            (?, ?)
        "#
    )
}

fn mysql_select_statement(table_name: &str) -> String {
    format!("SELECT user_id, name FROM {table_name} ORDER BY user_id ASC")
}

fn mysql_get_values(count: usize) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = format!("{i:03}");
        let name = format!("name-{}", Uuid::new_v4());

        values.push((user_id, name));
    }
    values
}

fn mysql_insert_statements(
    table_name: &str,
    values: Vec<(String, String)>,
    expected: Option<u64>,
) -> Vec<StatementTest> {
    let mut insert_tests: Vec<StatementTest> = Vec::with_capacity(values.len());
    for (user_id, name) in values {
        let params: Vec<String> = vec![user_id.to_string(), name];

        insert_tests.push(StatementTest::execute_test(
            mysql_insert_statement(table_name),
            params.clone(),
            expected,
        ));
    }
    insert_tests
}

fn mysql_select_statements(
    table_name: &str,
    expected_values: Vec<(String, String)>,
) -> Vec<StatementTest> {
    let expected = mysql_get_expected(expected_values);
    let select_test1 = StatementTest::query_stream_test(
        mysql_select_statement(table_name),
        vec![],
        Some(expected.clone()),
    );
    let select_test2 = StatementTest::query_test(
        mysql_select_statement(table_name),
        vec![],
        Some(expected.clone()),
    );

    vec![select_test1.clone(), select_test2.clone()]
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
    query_response(expected_columns, expected_rows)
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let test1 = StatementTest::execute_test("SELECT 1".to_string(), vec![], Some(0));

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
    let expected = query_response(expected_columns, expected_rows);

    let test2 = StatementTest::query_test("SELECT 1".to_string(), vec![], Some(expected));

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        &mysql.public_connection_string(),
        RdbmsTest::new(vec![test1, test2], None),
        1,
    )
    .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_transaction_repo_create_table_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let db_address = mysql.public_connection_string();
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;

    let worker_ids =
        start_workers::<MysqlType>(&executor, &component.id, &db_address, "", 1).await?;

    let worker_id = worker_ids[0].clone();

    let create_read_user_test = RdbmsTest::new(
        vec![
            StatementTest::execute_test(
                "CREATE USER 'global_reader'@'%' IDENTIFIED BY 'SomeSecurePass!';".to_string(),
                vec![],
                None,
            ),
            StatementTest::execute_test(
                "GRANT SELECT ON *.* TO 'global_reader'@'%';".to_string(),
                vec![],
                None,
            ),
            StatementTest::execute_test("FLUSH PRIVILEGES;".to_string(), vec![], None),
        ],
        None,
    );

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        create_read_user_test.clone(),
    )
    .await?;

    check_test_result(&worker_id, result1.clone(), create_read_user_test.clone());

    let db_address = mysql.public_connection_string_with_user("global_reader", "SomeSecurePass!");

    let worker_ids =
        start_workers::<MysqlType>(&executor, &component.id, &db_address, "", 1).await?;

    let worker_id = worker_ids[0].clone();

    let test = RdbmsTest::new(
        vec![StatementTest::execute_test(
            "SELECT 1".to_string(),
            vec![],
            Some(0),
        )],
        None,
    );

    let result1 = execute_worker_test::<MysqlType>(
        &executor,
        &worker_id,
        &IdempotencyKey::fresh(),
        test.clone(),
    )
    .await?;

    let error = get_test_result_error(result1);

    // Error::Other("There was a problem to create 'golem_transactions' table, see: https://learn.golem.cloud/common-language-guide/rdbms for more details (error: 1142 (42000): CREATE command denied to user 'global_reader'@'192.168.65.1' for table 'golem_transactions')")
    check!(error.contains("There was a problem to create 'golem_transactions' table"));

    Ok(())
}

async fn rdbms_component_test<T: RdbmsType>(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_address: &str,
    test: RdbmsTest,
    n_workers: u8,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "rdbms-service")
        .store()
        .await?;
    let worker_ids =
        start_workers::<T>(&executor, &component.id, db_address, "", n_workers).await?;

    rdbms_workers_test::<T>(&executor, worker_ids, test).await?;

    Ok(())
}

async fn rdbms_workers_test<T: RdbmsType>(
    executor: &TestWorkerExecutor,
    worker_ids: Vec<WorkerId>,
    test: RdbmsTest,
) -> anyhow::Result<()> {
    let mut workers_results: HashMap<WorkerId, Result<Option<ValueAndType>, WorkerExecutorError>> =
        HashMap::new(); // <worker_id, results>

    let mut fibers = JoinSet::<
        anyhow::Result<(WorkerId, Result<Option<ValueAndType>, WorkerExecutorError>)>,
    >::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let executor_clone = executor.clone();
        let test_clone = test.clone();
        let _ = fibers.spawn(
            async move {
                let result = execute_worker_test::<T>(
                    &executor_clone,
                    &worker_id_clone,
                    &IdempotencyKey::fresh(),
                    test_clone,
                )
                .await?;
                Ok((worker_id_clone, result))
            }
            .in_current_span(),
        );
    }

    while let Some(res) = fibers.join_next().await {
        let (worker_id, result_execute) = res??;
        workers_results.insert(worker_id, result_execute);
    }

    for worker_id in workers_results.keys() {
        executor.check_oplog_is_queryable(worker_id).await?;
    }

    for (worker_id, result) in workers_results {
        check_test_result(&worker_id, result, test.clone());
    }

    Ok(())
}

async fn execute_worker_test<T: RdbmsType>(
    executor: &TestWorkerExecutor,
    worker_id: &WorkerId,
    idempotency_key: &IdempotencyKey,
    test: RdbmsTest,
) -> anyhow::Result<Result<Option<ValueAndType>, WorkerExecutorError>> {
    let db_type = T::default().to_string();

    let fn_name = test.fn_name();
    let component_fn_name = format!("golem:it/api.{{{db_type}-{fn_name}}}");

    let mut statements: Vec<Value> = Vec::with_capacity(test.statements.len());

    for s in test.statements {
        let params = Value::List(s.params.into_iter().map(Value::String).collect());
        statements.push(Value::Record(vec![
            Value::String(s.statement),
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
    result: Result<Option<ValueAndType>, WorkerExecutorError>,
    test: RdbmsTest,
) {
    let fn_name = test.fn_name();

    check!(
        result.is_ok(),
        "result {fn_name} for worker {worker_id} is ok"
    );

    let response = result
        .unwrap()
        .map(|response| serde_json::to_value(response).unwrap())
        .and_then(|response| response.get("value").cloned());

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

fn get_test_result_error(result: Result<Option<ValueAndType>, WorkerExecutorError>) -> String {
    check!(result.is_ok());

    let response = result
        .unwrap()
        .map(|response| serde_json::to_value(response).unwrap())
        .and_then(|response| response.get("value").cloned());

    let err_response = response
        .and_then(|v| v.get("err").cloned())
        .and_then(|v| v.as_str().map(|v| v.to_string()));

    check!(err_response.is_some());

    err_response.unwrap()
}

async fn start_workers<T: RdbmsType>(
    executor: &TestWorkerExecutor,
    component_id: &ComponentId,
    db_address: &str,
    name_suffix: &str,
    n_workers: u8,
) -> anyhow::Result<Vec<WorkerId>> {
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let db_type = T::default().to_string();

    let mut env = HashMap::new();
    let db_env_var = format!("DB_{}_URL", db_type.to_uppercase());
    env.insert(db_env_var, db_address.to_string());

    for _ in 0..n_workers {
        let worker_name = format!(
            "rdbms-service-{}-{}{}",
            db_type,
            Uuid::new_v4(),
            name_suffix
        );
        let worker_id = executor
            .start_worker_with(component_id, &worker_name, env.clone(), vec![])
            .await?;
        worker_ids.push(worker_id.clone());
        let _result = executor
            .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
            .await??;
    }
    Ok(worker_ids)
}

async fn workers_interrupt_test(
    executor: &TestWorkerExecutor,
    worker_ids: Vec<WorkerId>,
) -> anyhow::Result<()> {
    let mut workers_results: HashMap<WorkerId, WorkerStatus> = HashMap::new();

    let mut fibers = JoinSet::<anyhow::Result<(WorkerId, WorkerStatus)>>::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let executor_clone = executor.clone();
        let _ = fibers.spawn(
            async move {
                executor_clone.interrupt(&worker_id_clone).await?;

                let metadata = executor_clone
                    .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(5))
                    .await?;

                let status = metadata.status;

                Ok((worker_id_clone, status))
            }
            .in_current_span(),
        );
    }

    while let Some(res) = fibers.join_next().await {
        let (worker_id, status) = res??;
        workers_results.insert(worker_id, status);
    }

    for (worker_id, status) in workers_results {
        check!(
            status == WorkerStatus::Idle,
            "status for worker {worker_id} is Idle"
        );
    }

    Ok(())
}

fn execute_response(value: u64) -> serde_json::Value {
    json!(
       {
           "execute": value
       }
    )
}

fn query_response(
    columns: Vec<serde_json::Value>,
    rows: Vec<serde_json::Value>,
) -> serde_json::Value {
    json!(
        {
         "query":{
             "columns": columns,
             "rows": rows
         }
       }
    )
}

fn query_empty_response() -> serde_json::Value {
    query_response(vec![], vec![])
}
