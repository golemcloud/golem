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

use std::collections::HashMap;
use std::fmt::Display;
use test_r::{inherit_test_dep, test, test_dep};

use crate::common::{start, TestContext, TestWorkerExecutor};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;
use golem_common::model::{ComponentId, WorkerId};
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdbs;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use golem_test_framework::components::rdb::RdbsConnections;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::Value;
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
pub enum StatementAction {
    Execute,
    Query,
}

impl Display for StatementAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatementAction::Execute => write!(f, "execute"),
            StatementAction::Query => write!(f, "query"),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum TransactionEnd {
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
    pub expected: Option<serde_json::Value>,
}

impl StatementTest {
    fn execute_test(statement: &'static str, params: Vec<String>, expected: Option<u64>) -> Self {
        Self {
            action: StatementAction::Execute,
            statement,
            params,
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
            expected,
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
    pub statements: Vec<StatementTest>,
    pub transaction_end: Option<TransactionEnd>,
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

    let mut expected_values: Vec<(Uuid, String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = Uuid::new_v4();
        let name = format!("name-{}", Uuid::new_v4());
        let vs: Vec<String> = (0..5).map(|v| format!("tag-{}-{}", v, i)).collect();
        let tags = format!("[{}]", vs.join(", "));

        let params: Vec<String> = vec![user_id.clone().to_string(), name.clone(), tags.clone()];

        insert_tests.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        expected_values.push((user_id, name, tags));
    }

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit)),
        1,
    )
    .await;

    fn get_row(columns: (Uuid, String, String)) -> serde_json::Value {
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

    fn get_expected(expected_values: Vec<(Uuid, String, String)>) -> serde_json::Value {
        let expected_rows: Vec<serde_json::Value> =
            expected_values.into_iter().map(get_row).collect();
        json!(
               {
                  "ok":{
                   "query":{
                     "columns":[
                        {
                           "db-type":{
                              "uuid":null
                           },
                           "db-type-name":"UUID",
                           "name":"user_id",
                           "ordinal":0
                        },
                        {
                           "db-type":{
                              "text":null
                           },
                           "db-type-name":"TEXT",
                           "name":"name",
                           "ordinal":1
                        },
                        {
                           "db-type":{
                                 "text":null
                           },
                           "db-type-name":"TEXT[]",
                           "name":"tags",
                           "ordinal":2
                        }
                     ],
                     "rows": expected_rows
                    }
                  }
               }
        )
    }

    let expected = get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_test(
        "SELECT user_id, name, tags FROM test_users ORDER BY created_on ASC",
        vec![],
        Some(expected),
    );

    let expected = get_expected(vec![expected_values[0].clone()]);
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name, tags FROM test_users WHERE user_id = $1::uuid ORDER BY created_on ASC",
        vec![expected_values[0].0.to_string()],
        Some(expected),
    );

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![select_test1.clone(), select_test2], None),
        3,
    )
    .await;

    let delete = StatementTest::execute_test("DELETE FROM test_users", vec![], None);

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Rollback)),
        1,
    )
    .await;

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::None)),
        1,
    )
    .await;

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![select_test1.clone()], Some(TransactionEnd::Commit)),
        3,
    )
    .await;

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Commit)),
        1,
    )
    .await;

    let select_test = select_test1.with_expected(Some(query_empty_ok_response()));

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![select_test], Some(TransactionEnd::Commit)),
        1,
    )
    .await;
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

    let expected = json!(
               {
                  "ok":{
                     "query":{
                         "columns":[
                            {
                               "db-type":{
                                     "int4":null
                               },
                               "db-type-name":"INT4",
                               "name":"?column?",
                               "ordinal":0
                            }
                         ],
                         "rows":[
                            {
                               "values":[
                                  {
                                      "int4":1
                                  }
                               ]
                            }
                         ]
                      }
                   }
               }
    );

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

    let mut expected_values: Vec<(String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = format!("{:03}", i);
        let name = format!("name-{}", Uuid::new_v4());

        let params: Vec<String> = vec![user_id.clone(), name.clone()];

        insert_tests.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        expected_values.push((user_id, name));
    }

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(insert_tests, Some(TransactionEnd::Commit)),
        1,
    )
    .await;

    fn get_row(columns: (String, String)) -> serde_json::Value {
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

    fn get_expected(expected_values: Vec<(String, String)>) -> serde_json::Value {
        let expected_rows: Vec<serde_json::Value> =
            expected_values.into_iter().map(get_row).collect();
        json!(
           {
              "ok":{
                "query":{
                 "columns":[
                    {
                       "db-type":{
                          "varchar":null
                       },
                       "db-type-name":"VARCHAR",
                       "name":"user_id",
                       "ordinal":0
                    },
                    {
                       "db-type":{
                          "varchar":null
                       },
                       "db-type-name":"VARCHAR",
                       "name":"name",
                       "ordinal":1
                    }
                 ],
                 "rows": expected_rows
                }
              }
           }
        )
    }

    let expected = get_expected(expected_values.clone());
    let select_test1 = StatementTest::query_test(
        "SELECT user_id, name FROM test_users ORDER BY user_id ASC",
        vec![],
        Some(expected),
    );

    let expected = get_expected(vec![expected_values[0].clone()]);
    let select_test2 = StatementTest::query_test(
        "SELECT user_id, name FROM test_users WHERE user_id = ? ORDER BY user_id ASC",
        vec![expected_values[0].clone().0],
        Some(expected),
    );

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![select_test1.clone(), select_test2], None),
        3,
    )
    .await;

    let delete = StatementTest::execute_test("DELETE FROM test_users", vec![], None);

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Rollback)),
        1,
    )
    .await;

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::None)),
        1,
    )
    .await;

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![select_test1.clone()], Some(TransactionEnd::Commit)),
        3,
    )
    .await;

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![delete.clone()], Some(TransactionEnd::Commit)),
        1,
    )
    .await;

    let select_test = select_test1.with_expected(Some(query_empty_ok_response()));

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        RdbmsTest::new(vec![select_test], Some(TransactionEnd::Commit)),
        1,
    )
    .await;
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
    let expected = json!(
       {
          "ok":{
             "query":{
                 "columns":[
                    {
                       "db-type":{
                          "bigint":null
                       },
                       "db-type-name":"BIGINT",
                       "name":"1",
                       "ordinal":0
                    }
                 ],
                 "rows":[
                    {
                       "values":[
                          {
                             "bigint":1
                          }
                       ]
                    }
                 ]
             }
           }
       }
    );

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

async fn rdbms_component_test<T: RdbmsType + Default + Display>(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_addresses: Vec<String>,
    test: RdbmsTest,
    workers_per_db: u8,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.store_component("rdbms-service").await;
    let worker_ids =
        start_workers::<T>(&executor, &component_id, db_addresses, workers_per_db).await;

    rdbms_workers_test::<T>(&executor, worker_ids, test).await;

    drop(executor);
}

async fn rdbms_workers_test<T: RdbmsType + Default + Display>(
    executor: &TestWorkerExecutor,
    worker_ids: Vec<WorkerId>,
    test: RdbmsTest,
) {
    let db_type = T::default().to_string();
    let mut workers_results: HashMap<WorkerId, Result<TypeAnnotatedValue, Error>> = HashMap::new(); // <worker_id, results>

    let mut fibers = JoinSet::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let executor_clone = executor.clone();
        let test_clone = test.clone();
        let db_type_clone = db_type.clone();
        let _ = fibers.spawn(async move {
            let fn_name = test_clone.fn_name();
            let component_fn_name = format!("golem:it/api.{{{db_type_clone}-{fn_name}}}");
            let mut statements: Vec<Value> = Vec::with_capacity(test_clone.statements.len());

            for s in test_clone.statements {
                let params = Value::List(s.params.into_iter().map(Value::String).collect());
                statements.push(Value::Record(vec![
                    Value::String(s.statement.to_string()),
                    params,
                    Value::Enum(s.action as u32),
                ]));
            }

            let mut fn_params: Vec<Value> = vec![Value::List(statements)];

            if let Some(te) = test_clone.transaction_end {
                fn_params.push(Value::Enum(te as u32));
            }

            let result = executor_clone
                .invoke_and_await_typed(&worker_id_clone, component_fn_name.as_str(), fn_params)
                .await;

            (worker_id_clone, result)
        });
    }

    while let Some(res) = fibers.join_next().await {
        let (worker_id, result_execute) = res.unwrap();
        workers_results.insert(worker_id, result_execute);
    }

    for (worker_id, result) in workers_results {
        let query_test = test.clone();
        let fn_name = test.fn_name();

        check!(
            result.is_ok(),
            "result {fn_name} for worker {worker_id} is error"
        );

        let response = result.clone().unwrap().to_json_value();

        let response = response
            .as_array()
            .and_then(|v| v.first())
            .and_then(|v| v.as_object())
            .cloned();

        if query_test.has_expected() {
            let ok_response = response
                .and_then(|v| v.get("ok").cloned())
                .and_then(|v| v.as_array().cloned());

            if let Some(response_values) = ok_response {
                for (index, test_statement) in query_test.statements.into_iter().enumerate() {
                    let action = test_statement.action;
                    if let Some(expected) = test_statement.expected {
                        match response_values.get(index).cloned() {
                            Some(response) => {
                                // println!("{}", response);
                                check!(
                                    response == expected,
                                    "result {fn_name} {action} with index {index} for worker {worker_id}"
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
}

async fn start_workers<T: RdbmsType + Default + Display>(
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

fn execute_ok_response(value: u64) -> serde_json::Value {
    json!(
       {
          "ok": {
              "execute": value
           }
       }
    )
}

fn query_empty_ok_response() -> serde_json::Value {
    json!(
        {
          "ok":{
             "query":{
                 "columns":[],
                 "rows":[]
             }
           }
       }
    )
}
