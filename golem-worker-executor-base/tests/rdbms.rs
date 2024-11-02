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
use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn rdbms_postgres_select1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("rdbms-service").await;
    let worker_name = "rdbms-service-1";

    let mut env = HashMap::new();
    env.insert(
        "DB_URL".to_string(),
        "postgresql://postgres:postgres@localhost:5444/postgres".to_string(),
    );

    let worker_id = executor
        .start_worker_with(&component_id, worker_name, vec![], env)
        .await;

    let _result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{check}",
            vec![],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{execute}",
            vec![Value::String(format!("SELECT 1;")), Value::List(vec![])],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::Result(Ok(Some(Box::new(Value::U64(1)))))]);
}
