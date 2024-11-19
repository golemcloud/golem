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

#[test]
#[tracing::instrument]
async fn rdbms_postgres_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdbs,
    _tracing: &Tracing,
) {
    rdbms_execute_test(
        last_unique_id,
        deps,
        postgres.host_connection_strings(),
        "postgres",
        "SELECT 1",
        vec![],
        Some(1),
    )
    .await;
    let expected = json!(
            [
               {
                  "ok":{
                     "columns":[
                        {
                           "db-type":{
                              "primitive":{
                                 "int32":null
                              }
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
                                 "primitive":{
                                    "int32":1
                                 }
                              }
                           ]
                        }
                     ]
                  }
               }
            ]
    );
    rdbms_query_test(
        last_unique_id,
        deps,
        postgres.host_connection_strings(),
        "postgres",
        "SELECT 1",
        vec![],
        Some(expected),
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
    rdbms_execute_test(
        last_unique_id,
        deps,
        mysql.host_connection_strings(),
        "mysql",
        "SELECT 1",
        vec![],
        Some(0),
    )
    .await;
    let expected = json!(
            [
               {
                  "ok":{
                     "columns":[
                        {
                           "db-type":{
                              "primitive":{
                                 "int64":null
                              }
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
                                 "primitive":{
                                    "int64":1
                                 }
                              }
                           ]
                        }
                     ]
                  }
               }
            ]
    );
    rdbms_query_test(
        last_unique_id,
        deps,
        mysql.host_connection_strings(),
        "mysql",
        "SELECT 1",
        vec![],
        Some(expected),
    )
    .await;
}

async fn rdbms_execute_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_addresses: Vec<String>,
    db_type: &'static str,
    query: &'static str,
    params: Vec<String>,
    expected: Option<u64>,
) {
    let expected = expected.map(execute_ok_response);

    rdbms_component_test(
        last_unique_id,
        deps,
        db_addresses,
        db_type,
        "execute",
        query,
        params,
        expected,
    )
    .await;
}

async fn rdbms_query_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_addresses: Vec<String>,
    db_type: &'static str,
    query: &'static str,
    params: Vec<String>,
    expected: Option<serde_json::Value>,
) {
    rdbms_component_test(
        last_unique_id,
        deps,
        db_addresses,
        db_type,
        "query",
        query,
        params,
        expected,
    )
    .await;
}

async fn rdbms_component_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    db_addresses: Vec<String>,
    db_type: &'static str,
    fn_name: &'static str,
    query: &'static str,
    params: Vec<String>,
    expected: Option<serde_json::Value>,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.store_component("rdbms-service").await;
    let worker_ids = start_workers(&executor, &component_id, db_addresses, db_type, 3).await;

    rdbms_workers_test(
        &executor, worker_ids, db_type, fn_name, query, params, expected,
    )
    .await;

    drop(executor);
}

async fn rdbms_workers_test(
    executor: &TestWorkerExecutor,
    worker_ids: Vec<WorkerId>,
    db_type: &'static str,
    fn_name: &'static str,
    query: &'static str,
    params: Vec<String>,
    expected: Option<serde_json::Value>,
) {
    let component_fn_name = format!("golem:it/api.{{{db_type}-{fn_name}}}");

    let params = Value::List(params.into_iter().map(Value::String).collect());

    let mut workers_results: HashMap<WorkerId, Result<TypeAnnotatedValue, Error>> = HashMap::new(); // <worker_id, result>

    let mut fibers = JoinSet::new();

    for worker_id in worker_ids {
        let worker_id_clone = worker_id.clone();
        let params_clone = params.clone();
        let executor_clone = executor.clone();
        let component_fn_name_clone = component_fn_name.clone();
        let _ = fibers.spawn(async move {
            let result = executor_clone
                .invoke_and_await_typed(
                    &worker_id_clone,
                    component_fn_name_clone.as_str(),
                    vec![Value::String(query.to_string()), params_clone],
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
        check!(
            result.is_ok(),
            "result {fn_name} for worker {worker_id} is error"
        );

        if let Some(expected) = expected.clone() {
            check!(
                result.unwrap().to_json_value() == expected,
                "result {fn_name} for worker {worker_id}"
            );
        }
    }
}

async fn start_workers(
    executor: &TestWorkerExecutor,
    component_id: &ComponentId,
    db_addresses: Vec<String>,
    db_type: &'static str,
    count: u8,
) -> Vec<WorkerId> {
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let db_env_var = format!("DB_{}_URL", db_type.to_uppercase());

    for db_address in db_addresses.into_iter() {
        let mut env = HashMap::new();
        env.insert(db_env_var.clone(), db_address);
        for _ in 0..count {
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
