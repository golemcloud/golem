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

#[derive(Debug, Clone)]
struct RdbmsTest {
    pub fn_name: &'static str,
    pub statement: &'static str,
    pub params: Vec<String>,
    pub expected: Option<serde_json::Value>,
}

impl RdbmsTest {
    fn query_test(
        statement: &'static str,
        params: Vec<String>,
        expected: Option<serde_json::Value>,
    ) -> Self {
        Self {
            fn_name: "query",
            statement,
            params,
            expected,
        }
    }

    fn execute_test(statement: &'static str, params: Vec<String>, expected: Option<u64>) -> Self {
        Self {
            fn_name: "execute",
            statement,
            params,
            expected: expected.map(execute_ok_response),
        }
    }
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_create_insert_select(
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
            ($1::uuid, $2, ARRAY[$3::text, $4::text])
        "#;

    let count = 60;

    let mut insert_tests: Vec<RdbmsTest> = Vec::with_capacity(count + 1);

    insert_tests.push(RdbmsTest::execute_test(
        create_table_statement,
        vec![],
        None,
    ));

    let mut expected_values: Vec<(Uuid, String, String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = Uuid::new_v4();
        let name = format!("name-{}", Uuid::new_v4());
        let tag1 = format!("tag-1-{}", i);
        let tag2 = format!("tag-2-{}", i);

        let params: Vec<String> = vec![
            user_id.clone().to_string(),
            name.clone(),
            tag1.clone(),
            tag2.clone(),
        ];

        insert_tests.push(RdbmsTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        expected_values.push((user_id, name, tag1, tag2));
    }

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        insert_tests,
        1,
    )
    .await;

    fn get_row(columns: (Uuid, String, String, String)) -> serde_json::Value {
        let user_id = columns.0.as_u64_pair();
        json!(
                    {
                       "values":[
                          {
                             "nodes":[{
                                "uuid":  {
                                   "high-bits": user_id.0,
                                   "low-bits": user_id.1
                                }
                              }]
                          },
                          {
                             "nodes":[{
                                "text": columns.1
                             }]
                          },
                          {
                             "nodes":[
                              {
                                "array": [1, 2]
                              },
                              {
                                "text": columns.2
                              },
                              {
                                "text": columns.3
                              }
                             ]
                          }
                       ]
                    }
        )
    }

    fn get_expected(expected_values: Vec<(Uuid, String, String, String)>) -> serde_json::Value {
        let expected_rows: Vec<serde_json::Value> =
            expected_values.into_iter().map(get_row).collect();
        json!(
            [
               {
                  "ok":{
                     "columns":[
                        {
                           "db-type":{
                              "nodes":[{
                                 "uuid":null
                              }]
                           },
                           "db-type-name":"UUID",
                           "name":"user_id",
                           "ordinal":0
                        },
                        {
                           "db-type":{
                              "nodes":[{
                                 "text":null
                              }]
                           },
                           "db-type-name":"TEXT",
                           "name":"name",
                           "ordinal":1
                        },
                        {
                           "db-type":{
                              "nodes":[
                              {

                                 "array": 1
                              },
                              {
                                 "text":null
                              }
                            ]
                           },
                           "db-type-name":"TEXT[]",
                           "name":"tags",
                           "ordinal":2
                        }
                     ],
                     "rows": expected_rows
                  }
               }
            ]
        )
    }

    let expected = get_expected(expected_values.clone());
    let select_test1 = RdbmsTest::query_test(
        "SELECT user_id, name, tags FROM test_users ORDER BY created_on ASC",
        vec![],
        Some(expected),
    );

    let expected = get_expected(vec![expected_values[0].clone()]);
    let select_test2 = RdbmsTest::query_test(
        "SELECT user_id, name, tags FROM test_users WHERE user_id = $1::uuid ORDER BY created_on ASC",
        vec![expected_values[0].0.to_string()],
        Some(expected),
    );

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        vec![select_test1, select_test2],
        3,
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
    let test1 = RdbmsTest::execute_test("SELECT 1", vec![], Some(1));

    let expected = json!(
            [
               {
                  "ok":{
                     "columns":[
                        {
                           "db-type":{
                              "nodes":[{
                                 "int4":null
                              }]
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
                                 "nodes":[{
                                    "int4":1
                                 }]
                              }
                           ]
                        }
                     ]
                  }
               }
            ]
    );

    let test2 = RdbmsTest::query_test("SELECT 1", vec![], Some(expected));

    rdbms_component_test::<PostgresType>(
        last_unique_id,
        deps,
        postgres.host_connection_strings(),
        vec![test1, test2],
        3,
    )
    .await;
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_create_insert_select(
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

    let mut insert_tests: Vec<RdbmsTest> = Vec::with_capacity(count + 1);

    insert_tests.push(RdbmsTest::execute_test(
        create_table_statement,
        vec![],
        None,
    ));

    let mut expected_values: Vec<(String, String)> = Vec::with_capacity(count);

    for i in 0..count {
        let user_id = format!("{:03}", i);
        let name = format!("name-{}", Uuid::new_v4());

        let params: Vec<String> = vec![user_id.clone(), name.clone()];

        insert_tests.push(RdbmsTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        expected_values.push((user_id, name));
    }

    rdbms_component_test::<MysqlType>(last_unique_id, deps, db_addresses.clone(), insert_tests, 1)
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
            [
               {
                  "ok":{
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
            ]
        )
    }

    let expected = get_expected(expected_values.clone());
    let select_test1 = RdbmsTest::query_test(
        "SELECT user_id, name FROM test_users ORDER BY user_id ASC",
        vec![],
        Some(expected),
    );

    let expected = get_expected(vec![expected_values[0].clone()]);
    let select_test2 = RdbmsTest::query_test(
        "SELECT user_id, name FROM test_users WHERE user_id = ? ORDER BY user_id ASC",
        vec![expected_values[0].clone().0],
        Some(expected),
    );

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        db_addresses.clone(),
        vec![select_test1, select_test2],
        3,
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
    let test1 = RdbmsTest::execute_test("SELECT 1", vec![], Some(0));
    let expected = json!(
            [
               {
                  "ok":{
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
            ]
    );

    let test2 = RdbmsTest::query_test("SELECT 1", vec![], Some(expected));

    rdbms_component_test::<MysqlType>(
        last_unique_id,
        deps,
        mysql.host_connection_strings(),
        vec![test1, test2],
        3,
    )
    .await;
}

async fn rdbms_component_test<T: RdbmsType + Default + Display>(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_addresses: Vec<String>,
    tests: Vec<RdbmsTest>,
    workers_per_db: u8,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.store_component("rdbms-service").await;
    let worker_ids =
        start_workers::<T>(&executor, &component_id, db_addresses, workers_per_db).await;

    rdbms_workers_test::<T>(&executor, worker_ids, tests).await;

    drop(executor);
}

async fn rdbms_workers_test<T: RdbmsType + Default + Display>(
    executor: &TestWorkerExecutor,
    worker_ids: Vec<WorkerId>,
    tests: Vec<RdbmsTest>,
) {
    let db_type = T::default().to_string();
    let mut workers_results: HashMap<WorkerId, HashMap<usize, Result<TypeAnnotatedValue, Error>>> =
        HashMap::new(); // <worker_id, results>

    let mut fibers = JoinSet::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let executor_clone = executor.clone();
        let tests_clone = tests.clone();
        let db_type_clone = db_type.clone();
        let _ = fibers.spawn(async move {
            let mut query_results: HashMap<usize, Result<TypeAnnotatedValue, Error>> =
                HashMap::new();
            for (index, query_test) in tests_clone.into_iter().enumerate() {
                let fn_name = query_test.fn_name;
                let component_fn_name = format!("golem:it/api.{{{db_type_clone}-{fn_name}}}");
                let params =
                    Value::List(query_test.params.into_iter().map(Value::String).collect());
                let result = executor_clone
                    .invoke_and_await_typed(
                        &worker_id_clone,
                        component_fn_name.as_str(),
                        vec![Value::String(query_test.statement.to_string()), params],
                    )
                    .await;

                query_results.insert(index, result);
            }
            (worker_id_clone, query_results)
        });
    }

    while let Some(res) = fibers.join_next().await {
        let (worker_id, result_execute) = res.unwrap();
        workers_results.insert(worker_id, result_execute);
    }

    for (worker_id, result) in workers_results {
        for (index, query_test) in tests.clone().into_iter().enumerate() {
            let fn_name = query_test.fn_name;

            match result.get(&index) {
                Some(result) => {
                    check!(
                        result.is_ok(),
                        "result {fn_name} with index {index} for worker {worker_id} is error"
                    );
                    if let Some(expected) = query_test.expected {
                        check!(
                            result.clone().unwrap().to_json_value() == expected,
                            "result {fn_name} with index {index} for worker {worker_id}"
                        );
                    };
                }
                _ => {
                    check!(
                        false,
                        "result {fn_name} with index {index} for worker {worker_id} is not found"
                    );
                }
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
        [
           {
              "ok": value
           }
        ]
    )
}
