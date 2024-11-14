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
use golem_wasm_rpc::Value;

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
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let component_id = executor.store_component("rdbms-service").await;

    for (db_index, rdb) in postgres.rdbs.iter().enumerate() {
        let db_address = rdb.host_connection_string();

        let mut env = HashMap::new();
        env.insert("DB_POSTGRES_URL".to_string(), db_address);

        for i in 0..3 {
            let worker_name = format!("rdbms-service-postgres-{db_index}-{i}");
            let worker_id = executor
                .start_worker_with(&component_id, &worker_name, vec![], env.clone())
                .await;
            worker_ids.push(worker_id.clone());
            let _result = executor
                .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
                .await
                .unwrap();
        }
    }

    let mut results_execute: HashMap<WorkerId, Result<Vec<Value>, Error>> = HashMap::new(); // <worker_id, result>

    let mut results_query: HashMap<WorkerId, Result<Vec<Value>, Error>> = HashMap::new(); // <worker_id, result>

    for worker_id in worker_ids {
        let result_execute: Result<Vec<Value>, Error> = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api.{postgres-execute}",
                vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
            )
            .await;

        results_execute.insert(worker_id.clone(), result_execute);
        let result_query = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api.{postgres-query}",
                vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
            )
            .await;

        results_query.insert(worker_id.clone(), result_query);
    }

    drop(executor);

    for (worker_id, result) in results_execute {
        check!(
            result.unwrap() == vec![Value::Result(Ok(Some(Box::new(Value::U64(1)))))],
            "result execute for worker {worker_id}"
        );
    }

    for (worker_id, result) in results_query {
        check!(
            result.is_ok(),
            "result query for worker {worker_id} is error"
        );
        // match result.unwrap()[0].clone() {
        //     Value::Result(Ok(Some(Box::new(Value::List(values))))) if !values.is_empty() => {},
        //     _ => {
        //         check!(false, "result query for worker {worker_id} not match");
        //     },
        // }

        // check!(result.unwrap() == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
    }
}

#[test]
#[tracing::instrument]
async fn rdbms_mysql_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    mysql: &DockerMysqlRdbs,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let mut worker_ids: Vec<WorkerId> = Vec::new();
    let component_id = executor.store_component("rdbms-service").await;

    for (db_index, rdb) in mysql.rdbs.iter().enumerate() {
        let db_address = rdb.host_connection_string();

        let mut env = HashMap::new();
        env.insert("DB_MYSQL_URL".to_string(), db_address);

        for i in 0..3 {
            let worker_name = format!("rdbms-service-mysql-{db_index}-{i}");
            let worker_id = executor
                .start_worker_with(&component_id, &worker_name, vec![], env.clone())
                .await;
            worker_ids.push(worker_id.clone());
            let _result = executor
                .invoke_and_await(&worker_id, "golem:it/api.{check}", vec![])
                .await
                .unwrap();
        }
    }

    let mut results_execute: HashMap<WorkerId, Result<Vec<Value>, Error>> = HashMap::new(); // <worker_id, result>

    let mut results_query: HashMap<WorkerId, Result<Vec<Value>, Error>> = HashMap::new(); // <worker_id, result>

    for worker_id in worker_ids {
        let result_execute: Result<Vec<Value>, Error> = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api.{mysql-execute}",
                vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
            )
            .await;

        results_execute.insert(worker_id.clone(), result_execute);
        let result_query = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api.{mysql-query}",
                vec![Value::String("SELECT 1;".to_string()), Value::List(vec![])],
            )
            .await;

        results_query.insert(worker_id.clone(), result_query);
    }

    drop(executor);

    for (worker_id, result) in results_execute {
        check!(
            result.unwrap() == vec![Value::Result(Ok(Some(Box::new(Value::U64(0)))))],
            "result execute for worker {worker_id} is error"
        );
    }

    for (worker_id, result) in results_query {
        check!(result.is_ok(), "result query for worker {worker_id}");
        // match result.unwrap()[0].clone() {
        //     Value::Result(Ok(Some(Box::new(Value::List(values))))) if !values.is_empty() => {},
        //     _ => {
        //         check!(false, "result query for worker {worker_id} not match");
        //     },
        // }
    }
}
