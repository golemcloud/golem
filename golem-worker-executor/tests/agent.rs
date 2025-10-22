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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::let_assert;
use assert2::{assert, check};
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;
use golem_api_grpc::proto::golem::worker::v1::{
    worker_execution_error, InvocationFailed, WorkerExecutionError,
};
use golem_api_grpc::proto::golem::worker::{UnknownError, WorkerError};
use golem_common::model::WorkerId;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm::{IntoValueAndType, Value};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn agent_self_rpc_is_not_allowed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor.component("golem_it_agent_self_rpc").store().await;
    let worker_id = executor
        .start_worker(&component_id, "self-rpc-agent(\"worker-name\")")
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-self-rpc/self-rpc-agent.{self-rpc}",
            vec![],
        )
        .await;

    let_assert!(
        Err(Error::InternalError(WorkerExecutionError {
            error: Some(worker_execution_error::Error::InvocationFailed(
                InvocationFailed {
                    error: Some(WorkerError {
                        error: Some(
                            golem_api_grpc::proto::golem::worker::worker_error::Error::UnknownError(
                                UnknownError {
                                    details: error_details
                                }
                            )
                        )
                    }),
                    ..
                }
            ))
        })) = result
    );
    assert!(error_details.contains("RPC calls to the same agent are not supported"));
}

#[test]
#[tracing::instrument]
async fn agent_await_parallel_rpc_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor
        .component("golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await;
    let unique_id = context.redis_prefix();
    let worker_id = executor
        .start_worker(&component_id, &format!("test-agent(\"{unique_id}\")"))
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-rpc/test-agent.{run}",
            vec![20f64.into_value_and_type()],
        )
        .await;

    executor.check_oplog_is_queryable(&worker_id).await;

    check!(result.is_ok());
}

#[test]
#[tracing::instrument]
async fn agent_env_inheritance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let component_id = executor
        .component("golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .with_env(vec![
            ("ENV1".to_string(), "1".to_string()),
            ("ENV2".to_string(), "2".to_string()),
        ])
        .unique()
        .store()
        .await;
    let unique_id = context.redis_prefix();

    let mut env = HashMap::new();
    env.insert("ENV2".to_string(), "22".to_string());
    env.insert("ENV3".to_string(), "33".to_string());

    let worker_id = executor
        .start_worker_with(
            &component_id,
            &format!("test-agent(\"{unique_id}\")"),
            vec![],
            env,
            vec![],
        )
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-rpc/test-agent.{env-var-test}",
            vec![],
        )
        .await;

    let child_worker_id = WorkerId {
        component_id: worker_id.component_id.clone(),
        worker_name: "child-agent(0)".to_string(),
    };

    executor.check_oplog_is_queryable(&worker_id).await;
    executor.check_oplog_is_queryable(&child_worker_id).await;

    let (mut child_metadata, _) = executor
        .get_worker_metadata(&child_worker_id)
        .await
        .expect("Could not get child metadata");

    child_metadata.env.sort_by_key(|(k, _)| k.clone());

    let mut parent_env_vars = BTreeMap::new();
    let mut child_env_vars = BTreeMap::new();

    if let Ok(results) = result {
        if let Some(Value::Record(fields)) = results.first() {
            let parent = &fields[0];
            let child = &fields[1];

            if let Value::List(parent_env_vars_list) = parent {
                for env_var in parent_env_vars_list {
                    if let Value::Record(env_var_kv) = env_var {
                        if let Value::String(key) = &env_var_kv[0] {
                            parent_env_vars.insert(key.clone(), env_var_kv[1].clone());
                        }
                    }
                }
            }

            if let Value::List(child_env_vars_list) = child {
                for env_var in child_env_vars_list {
                    if let Value::Record(env_var_kv) = env_var {
                        if let Value::String(key) = &env_var_kv[0] {
                            child_env_vars.insert(key.clone(), env_var_kv[1].clone());
                        }
                    }
                }
            }
        }
    }

    assert_eq!(
        parent_env_vars.into_iter().collect::<Vec<_>>(),
        vec![
            ("ENV1".to_string(), Value::String("1".to_string())),
            ("ENV2".to_string(), Value::String("22".to_string())),
            ("ENV3".to_string(), Value::String("33".to_string())),
            (
                "GOLEM_AGENT_ID".to_string(),
                Value::String(worker_id.worker_name.to_string())
            ),
            (
                "GOLEM_AGENT_TYPE".to_string(),
                Value::String("TestAgent".to_string())
            ),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                Value::String(worker_id.component_id.to_string())
            ),
            (
                "GOLEM_COMPONENT_VERSION".to_string(),
                Value::String("0".to_string())
            ),
            (
                "GOLEM_WORKER_NAME".to_string(),
                Value::String(worker_id.worker_name.to_string())
            ),
        ]
    );
    assert_eq!(
        child_env_vars.into_iter().collect::<Vec<_>>(),
        vec![
            ("ENV1".to_string(), Value::String("1".to_string())),
            ("ENV2".to_string(), Value::String("22".to_string())),
            ("ENV3".to_string(), Value::String("33".to_string())),
            (
                "GOLEM_AGENT_ID".to_string(),
                Value::String(child_worker_id.worker_name.to_string())
            ),
            (
                "GOLEM_AGENT_TYPE".to_string(),
                Value::String("ChildAgent".to_string())
            ),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                Value::String(child_worker_id.component_id.to_string())
            ),
            (
                "GOLEM_COMPONENT_VERSION".to_string(),
                Value::String("0".to_string())
            ),
            (
                "GOLEM_WORKER_NAME".to_string(),
                Value::String(child_worker_id.worker_name.to_string())
            ),
        ]
    );
    assert_eq!(
        child_metadata.env,
        vec![
            ("ENV1".to_string(), "1".to_string()),
            ("ENV2".to_string(), "22".to_string()),
            ("ENV3".to_string(), "33".to_string()),
            (
                "GOLEM_AGENT_ID".to_string(),
                child_worker_id.worker_name.to_string()
            ),
            ("GOLEM_AGENT_TYPE".to_string(), "ChildAgent".to_string()),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                child_worker_id.component_id.to_string()
            ),
            ("GOLEM_COMPONENT_VERSION".to_string(), "0".to_string()),
            (
                "GOLEM_WORKER_NAME".to_string(),
                child_worker_id.worker_name.to_string()
            ),
        ]
    );
}
