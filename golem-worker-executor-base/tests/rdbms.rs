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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;
use golem_common::model::WorkerId;
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdbs;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use golem_test_framework::components::rdb::RdbConnectionString;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::Value;
use serde_json::json;
use std::ops::Deref;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test_dep]
async fn postgres() -> DockerPostgresRdbs {
    DockerPostgresRdbs::new(3, 5444, true, false).await
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
    tracing: &Tracing,
) {
    rdbms_execute_test(
        last_unique_id,
        deps,
        tracing,
        postgres.rdbs[0].deref(),
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
        tracing,
        postgres.rdbs[0].deref(),
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
    tracing: &Tracing,
) {
    rdbms_execute_test(
        last_unique_id,
        deps,
        tracing,
        mysql.rdbs[0].deref(),
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
        tracing,
        mysql.rdbs[0].deref(),
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
    _tracing: &Tracing,
    rdb: &impl RdbConnectionString,
    db_type: &'static str,
    query: &'static str,
    params: Vec<String>,
    expected: Option<u64>,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let component_id = executor.store_component("rdbms-service").await;

    let db_env_var = format!("DB_{}_URL", db_type.to_uppercase());
    let execute_fn_name = format!("golem:it/api.{{{db_type}-execute}}");

    let wc = 3;
    let db_address = rdb.host_connection_string();

    let mut env = HashMap::new();
    env.insert(db_env_var.clone(), db_address);

    for i in 0..wc {
        let worker_name = format!("rdbms-service-{db_type}-{i}");
        let worker_id = executor
            .start_worker_with(&component_id, &worker_name, vec![], env.clone())
            .await;
        worker_ids.push(worker_id.clone());
        let _result = executor
            .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
            .await
            .unwrap();
    }

    let mut results_execute: HashMap<WorkerId, Result<Vec<Value>, Error>> = HashMap::new(); // <worker_id, result>

    let params = Value::List(params.into_iter().map(Value::String).collect());

    for worker_id in worker_ids {
        let result_execute: Result<Vec<Value>, Error> = executor
            .invoke_and_await(
                &worker_id,
                execute_fn_name.as_str(),
                vec![Value::String(query.to_string()), params.clone()],
            )
            .await;
        results_execute.insert(worker_id.clone(), result_execute);
    }

    drop(executor);

    for (worker_id, result) in results_execute {
        check!(
            result.is_ok(),
            "result execute for worker {worker_id} is error"
        );

        if let Some(expected) = expected {
            check!(
                result.unwrap() == vec![Value::Result(Ok(Some(Box::new(Value::U64(expected)))))],
                "result execute for worker {worker_id}"
            );
        }
    }
}

async fn rdbms_query_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    rdb: &impl RdbConnectionString,
    db_type: &'static str,
    query: &'static str,
    params: Vec<String>,
    expected: Option<serde_json::Value>,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let component_id = executor.store_component("rdbms-service").await;

    let db_env_var = format!("DB_{}_URL", db_type.to_uppercase());
    let query_fn_name = format!("golem:it/api.{{{db_type}-query}}");

    let wc = 3;
    let db_address = rdb.host_connection_string();

    let mut env = HashMap::new();
    env.insert(db_env_var.clone(), db_address);

    for i in 0..wc {
        let worker_name = format!("rdbms-service-{db_type}-{i}");
        let worker_id = executor
            .start_worker_with(&component_id, &worker_name, vec![], env.clone())
            .await;
        worker_ids.push(worker_id.clone());
        let _result = executor
            .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
            .await
            .unwrap();
    }

    let mut results_query: HashMap<WorkerId, Result<TypeAnnotatedValue, Error>> = HashMap::new(); // <worker_id, result>

    let params = Value::List(params.into_iter().map(Value::String).collect());

    for worker_id in worker_ids {
        let result_execute = executor
            .invoke_and_await_typed(
                &worker_id,
                query_fn_name.as_str(),
                vec![Value::String(query.to_string()), params.clone()],
            )
            .await;
        results_query.insert(worker_id.clone(), result_execute);
    }

    drop(executor);

    for (worker_id, result) in results_query {
        check!(
            result.is_ok(),
            "result query for worker {worker_id} is error"
        );

        if let Some(expected) = expected.clone() {
            check!(
                result.unwrap().to_json_value() == expected,
                "result query for worker {worker_id}"
            );
        }
    }
}
