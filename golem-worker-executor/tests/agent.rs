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

use golem_common::model::WorkerId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
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
        .component(&context.default_environment_id, "golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await?;
    let agent_id = agent_id!("self-rpc-agent", "worker-name");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "selfRpc", data_value!())
        .await;

    let err = result.expect_err("Expected an error");
    assert!(err
        .to_string()
        .contains("RPC calls to the same agent are not supported"));

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
    let agent_id = agent_id!("test-agent", unique_id);
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!(20f64))
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result.is_ok());
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
    let agent_id = agent_id!("test-agent", unique_id);

    let mut env = HashMap::new();
    env.insert("ENV2".to_string(), "22".to_string());
    env.insert("ENV3".to_string(), "33".to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, vec![])
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "env-var-test", data_value!())
        .await;

    let child_worker_id = WorkerId {
        component_id: worker_id.component_id,
        worker_name: "child-agent(0)".to_string(),
    };

    executor.check_oplog_is_queryable(&worker_id).await?;
    executor.check_oplog_is_queryable(&child_worker_id).await?;

    let child_metadata = executor.get_worker_metadata(&child_worker_id).await?;

    let mut parent_env_vars = BTreeMap::new();
    let mut child_env_vars = BTreeMap::new();

    if let Ok(data_value) = result {
        if let Some(Value::Record(fields)) = data_value.into_return_value().as_ref() {
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

    let agent_id1 = agent_id!("ephemeral-echo-agent", "param1");
    let worker_id1 = executor
        .start_agent(&component.id, agent_id1.clone())
        .await?;

    let agent_id2 = agent_id!("ephemeral-echo-agent", "param2");
    let worker_id2 = executor
        .start_agent(&component.id, agent_id2.clone())
        .await?;

    executor.log_output(&worker_id1).await?;
    executor.log_output(&worker_id2).await?;

    let result1 = executor
        .invoke_and_await_agent(&component.id, &agent_id1, "changeAndGet", data_value!())
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result2 = executor
        .invoke_and_await_agent(&component.id, &agent_id1, "changeAndGet", data_value!())
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result3 = executor
        .invoke_and_await_agent(&component.id, &agent_id2, "changeAndGet", data_value!())
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result4 = executor
        .invoke_and_await_agent(&component.id, &agent_id2, "changeAndGet", data_value!())
        .await?
        .into_return_value()
        .expect("Expected a return value");

    // As the agent is ephemeral, no matter how many times we call changeAndGet it always starts from scratch (no additional '!' suffix)
    assert_eq!(result1, Value::String("param1!".to_string()));
    assert_eq!(result2, Value::String("param1!".to_string()));
    assert_eq!(result3, Value::String("param2!".to_string()));
    assert_eq!(result4, Value::String("param2!".to_string()));
    Ok(())
}
