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
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdb;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test_dep]
async fn postgres() -> DockerPostgresRdb {
    DockerPostgresRdb::new(true, false, Some(5445)).await
}

#[test_dep]
async fn mysql() -> DockerMysqlRdb {
    DockerMysqlRdb::new(true, false, Some(3308)).await
}

#[test]
#[tracing::instrument]
async fn rdbms_postgres_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres: &DockerPostgresRdb,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let db_address = postgres.postgres_info().host_connection_string();

    let component_id = executor.store_component("rdbms-service").await;
    let worker_name = "rdbms-service-1";

    let mut env = HashMap::new();
    env.insert("DB_POSTGRES_URL".to_string(), db_address);

    let worker_id = executor
        .start_worker_with(&component_id, worker_name, vec![], env)
        .await;

    let _result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
        .await
        .unwrap();

    let result_execute = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{postgres-execute}",
            vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
        )
        .await
        .unwrap();

    let result_query = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{postgres-query}",
            vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result_execute == vec![Value::Result(Ok(Some(Box::new(Value::U64(1)))))]);

    check!(result_query == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdb,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let db_address = mysql.mysql_info().host_connection_string();

    let component_id = executor.store_component("rdbms-service").await;
    let worker_name = "rdbms-service-2";

    let mut env = HashMap::new();
    env.insert("DB_MYSQL_URL".to_string(), db_address);

    let worker_id = executor
        .start_worker_with(&component_id, worker_name, vec![], env)
        .await;

    let _result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
        .await
        .unwrap();

    let result_execute = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{mysql-execute}",
            vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
        )
        .await
        .unwrap();

    let result_query = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{mysql-query}",
            vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result_execute == vec![Value::Result(Ok(Some(Box::new(Value::U64(0)))))]);

    check!(result_query == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
}
