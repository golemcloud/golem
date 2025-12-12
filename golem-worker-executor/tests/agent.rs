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
use assert2::let_assert;
use assert2::{assert, check};
use golem_common::model::oplog::WorkerError;
use golem_common::model::WorkerId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValueAndType, Value};
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
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
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "golem_it_agent_self_rpc")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "self-rpc-agent(\"worker-name\")")
        .await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-self-rpc/self-rpc-agent.{self-rpc}",
            vec![],
        )
        .await?;

    let_assert!(
        Err(WorkerExecutorError::InvocationFailed {
            error: WorkerError::Unknown(error_details),
            ..
        }) = result
    );
    assert!(error_details.contains("RPC calls to the same agent are not supported"));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_await_parallel_rpc_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let worker_id = executor
        .start_worker(&component.id, &format!("test-agent(\"{unique_id}\")"))
        .await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-rpc/test-agent.{run}",
            vec![20f64.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(result.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_env_inheritance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .with_env(vec![
            ("ENV1".to_string(), "1".to_string()),
            ("ENV2".to_string(), "2".to_string()),
        ])
        .store()
        .await?;
    let unique_id = context.redis_prefix();

    let mut env = HashMap::new();
    env.insert("ENV2".to_string(), "22".to_string());
    env.insert("ENV3".to_string(), "33".to_string());

    let worker_id = executor
        .start_worker_with(
            &component.id,
            &format!("test-agent(\"{unique_id}\")"),
            env,
            vec![],
        )
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-rpc/test-agent.{env-var-test}",
            vec![],
        )
        .await?;

    let child_worker_id = WorkerId {
        component_id: worker_id.component_id,
        worker_name: "child-agent(0)".to_string(),
    };

    executor.check_oplog_is_queryable(&worker_id).await?;
    executor.check_oplog_is_queryable(&child_worker_id).await?;

    let child_metadata = executor.get_worker_metadata(&child_worker_id).await?;

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
                "GOLEM_COMPONENT_REVISION".to_string(),
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
                "GOLEM_COMPONENT_REVISION".to_string(),
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
        HashMap::from_iter(vec![
            ("ENV1".to_string(), "1".to_string()),
            ("ENV2".to_string(), "22".to_string()),
            ("ENV3".to_string(), "33".to_string()),
        ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_agent_works(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_constructor_parameter_echo",
        )
        .store()
        .await?;

    let worker_id1 = executor
        .start_worker(&component.id, "ephemeral-echo-agent(\"param1\")")
        .await?;

    let worker_id2 = executor
        .start_worker(&component.id, "ephemeral-echo-agent(\"param2\")")
        .await?;

    executor.log_output(&worker_id1).await?;
    executor.log_output(&worker_id2).await?;

    let result1 = executor
        .invoke_and_await(
            &worker_id1,
            "golem-it:constructor-parameter-echo/ephemeral-echo-agent.{change-and-get}",
            vec![],
        )
        .await??;

    let result2 = executor
        .invoke_and_await(
            &worker_id1,
            "golem-it:constructor-parameter-echo/ephemeral-echo-agent.{change-and-get}",
            vec![],
        )
        .await??;

    let result3 = executor
        .invoke_and_await(
            &worker_id2,
            "golem-it:constructor-parameter-echo/ephemeral-echo-agent.{change-and-get}",
            vec![],
        )
        .await??;

    let result4 = executor
        .invoke_and_await(
            &worker_id2,
            "golem-it:constructor-parameter-echo/ephemeral-echo-agent.{change-and-get}",
            vec![],
        )
        .await??;

    // As the agent is ephemeral, no matter how many times we call change-and-get it always starts from scratch (no additional '!' suffix)
    assert_eq!(result1, vec![Value::String("param1!".to_string())]);
    assert_eq!(result2, vec![Value::String("param1!".to_string())]);
    assert_eq!(result3, vec![Value::String("param2!".to_string())]);
    assert_eq!(result4, vec![Value::String("param2!".to_string())]);
    Ok(())
}
