// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_api_grpc::proto::golem::workerexecutor;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentId, IdempotencyKey, InvocationStatus};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("constructor_parameter_echo_unnamed")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn agent_self_rpc_is_not_allowed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let agent_id = agent_id!("SelfRpcAgent", "worker-name");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "selfRpc", data_value!())
        .await;

    let err = result.expect_err("Expected an error");
    assert!(
        err.to_string()
            .contains("RPC calls to the same agent are not supported")
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let scope_starts: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.request.is_none() => Some(entry.oplog_index),
            _ => None,
        })
        .collect();
    for start_index in scope_starts {
        assert!(
            oplog.iter().any(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == start_index
                )
            }),
            "durable scope opened at {start_index} was not closed"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_await_parallel_rpc_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_id!("TestAgent", unique_id);
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!(20f64))
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
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .with_env(
            "TestAgent",
            vec![
                ("ENV1".to_string(), "1".to_string()),
                ("ENV2".to_string(), "2".to_string()),
            ],
        )
        .store()
        .await?;
    let unique_id = context.redis_prefix();
    let agent_id = agent_id!("TestAgent", unique_id);

    let mut env = HashMap::new();
    env.insert("ENV2".to_string(), "22".to_string());
    env.insert("ENV3".to_string(), "33".to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "envVarTest", data_value!())
        .await;

    let child_worker_id = AgentId {
        component_id: worker_id.component_id,
        agent_id: "ChildAgent(0.0)".to_string(),
    };

    executor.check_oplog_is_queryable(&worker_id).await?;
    executor.check_oplog_is_queryable(&child_worker_id).await?;

    let child_metadata = executor.get_worker_metadata(&child_worker_id).await?;

    let mut parent_env_vars = BTreeMap::new();
    let mut child_env_vars = BTreeMap::new();

    if let Ok(data_value) = result
        && let Some(SchemaValue::Record { fields }) = data_value.into_return_value().as_ref()
    {
        let parent = &fields[0];
        let child = &fields[1];

        if let SchemaValue::List {
            elements: parent_env_vars_list,
        } = parent
        {
            for env_var in parent_env_vars_list {
                if let SchemaValue::Record { fields: env_var_kv } = env_var
                    && let SchemaValue::String(key) = &env_var_kv[0]
                {
                    parent_env_vars.insert(key.clone(), env_var_kv[1].clone());
                }
            }
        }

        if let SchemaValue::List {
            elements: child_env_vars_list,
        } = child
        {
            for env_var in child_env_vars_list {
                if let SchemaValue::Record { fields: env_var_kv } = env_var
                    && let SchemaValue::String(key) = &env_var_kv[0]
                {
                    child_env_vars.insert(key.clone(), env_var_kv[1].clone());
                }
            }
        }
    }

    assert_eq!(
        parent_env_vars.into_iter().collect::<Vec<_>>(),
        vec![
            ("ENV1".to_string(), SchemaValue::String("1".to_string())),
            ("ENV2".to_string(), SchemaValue::String("22".to_string())),
            ("ENV3".to_string(), SchemaValue::String("33".to_string())),
            (
                "GOLEM_AGENT_ID".to_string(),
                SchemaValue::String(worker_id.agent_id.to_string())
            ),
            (
                "GOLEM_AGENT_TYPE".to_string(),
                SchemaValue::String("TestAgent".to_string())
            ),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                SchemaValue::String(worker_id.component_id.to_string())
            ),
            (
                "GOLEM_COMPONENT_REVISION".to_string(),
                SchemaValue::String("0".to_string())
            ),
            (
                "GOLEM_WORKER_NAME".to_string(),
                SchemaValue::String(worker_id.agent_id.to_string())
            ),
        ]
    );
    assert_eq!(
        child_env_vars.into_iter().collect::<Vec<_>>(),
        vec![
            ("ENV1".to_string(), SchemaValue::String("1".to_string())),
            ("ENV2".to_string(), SchemaValue::String("22".to_string())),
            ("ENV3".to_string(), SchemaValue::String("33".to_string())),
            (
                "GOLEM_AGENT_ID".to_string(),
                SchemaValue::String(child_worker_id.agent_id.to_string())
            ),
            (
                "GOLEM_AGENT_TYPE".to_string(),
                SchemaValue::String("ChildAgent".to_string())
            ),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                SchemaValue::String(child_worker_id.component_id.to_string())
            ),
            (
                "GOLEM_COMPONENT_REVISION".to_string(),
                SchemaValue::String("0".to_string())
            ),
            (
                "GOLEM_WORKER_NAME".to_string(),
                SchemaValue::String(child_worker_id.agent_id.to_string())
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
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;

    let agent_id1 = agent_id!("EphemeralEchoAgent", "param1");
    let agent_id2 = agent_id!("EphemeralEchoAgent", "param2");
    let idempotency_key = IdempotencyKey::fresh();
    let result1 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id1,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result2 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id1,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id2, "changeAndGet", data_value!())
        .await?
        .into_typed::<String>()?;

    let result4 = executor
        .invoke_and_await_agent(&component, &agent_id2, "changeAndGet", data_value!())
        .await?
        .into_typed::<String>()?;

    assert_eq!(result1, "param1!");
    assert_eq!(result2, "param1!");
    assert_eq!(result3, "param2!");
    assert_eq!(result4, "param2!");
    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn immediate_scheduled_ephemeral_invocation_reuses_completed_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let logical_agent_id = agent_id!("EphemeralEchoAgent", "immediate-schedule-retry");
    let idempotency_key = IdempotencyKey::fresh();

    let initial_result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &logical_agent_id,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(initial_result, "immediate-schedule-retry!");

    let final_agent_id = logical_agent_id
        .with_ephemeral_invocation_phantom(&idempotency_key)
        .map_err(anyhow::Error::msg)?;
    let worker_id =
        AgentId::from_agent_id(component.id, &final_agent_id).map_err(anyhow::Error::msg)?;

    executor
        .client
        .clone()
        .invoke_agent(workerexecutor::v1::InvokeAgentRequest {
            agent_id: Some(worker_id.into()),
            method_name: Some("changeAndGet".to_string()),
            method_parameters: Some(SchemaValue::Tuple { elements: vec![] }.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition: workerexecutor::v1::InvocationFreshnessDisposition::MayExist
                as i32,
            config: Vec::new(),
            scope_card: None,
        })
        .await?;

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn ephemeral_invocation_lookup_does_not_create_unknown_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let final_agent_id = agent_id!("EphemeralEchoAgent", "unknown-lookup")
        .with_ephemeral_invocation_phantom(&idempotency_key)
        .map_err(anyhow::Error::msg)?;
    let worker_id =
        AgentId::from_agent_id(component.id, &final_agent_id).map_err(anyhow::Error::msg)?;

    let response = executor
        .client
        .clone()
        .invoke_agent(workerexecutor::v1::InvokeAgentRequest {
            agent_id: Some(worker_id.clone().into()),
            method_name: None,
            method_parameters: None,
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition: workerexecutor::v1::InvocationFreshnessDisposition::MayExist
                as i32,
            config: Vec::new(),
            scope_card: None,
        })
        .await?
        .into_inner();

    let success = match response.result {
        Some(workerexecutor::v1::invoke_agent_response::Result::Success(success)) => success,
        other => anyhow::bail!("unexpected lookup response: {other:?}"),
    };
    assert_eq!(
        success.status,
        Some(
            golem_api_grpc::proto::golem::worker::InvocationStatus::from(InvocationStatus::Unknown,)
                as i32
        )
    );
    assert_eq!(executor.get_worker_metadata_opt(&worker_id).await?, None);

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn scheduled_ephemeral_invocation_uses_schedule_time_component_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let final_agent_id = agent_id!("EphemeralEchoAgent", "scheduled-revision")
        .with_ephemeral_invocation_phantom(&idempotency_key)
        .map_err(anyhow::Error::msg)?;
    let worker_id =
        AgentId::from_agent_id(component.id, &final_agent_id).map_err(anyhow::Error::msg)?;

    executor
        .client
        .clone()
        .invoke_agent(workerexecutor::v1::InvokeAgentRequest {
            agent_id: Some(worker_id.clone().into()),
            method_name: Some("changeAndGet".to_string()),
            method_parameters: Some(SchemaValue::Tuple { elements: vec![] }.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: Some(prost_types::Timestamp {
                seconds: chrono::Utc::now().timestamp() + 3,
                nanos: 0,
            }),
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition: workerexecutor::v1::InvocationFreshnessDisposition::MayExist
                as i32,
            config: Vec::new(),
            scope_card: None,
        })
        .await?;

    let updated_component = executor
        .update_component(&component.id, &constructor_parameter_echo_unnamed.wasm_name)
        .await?;
    assert_ne!(component.revision, updated_component.revision);

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.component_revision, component.revision);

    Ok(())
}

/// Verifies that `AgentMode` is persisted in the `Create` oplog entry of a Durable agent and that
/// the worker is accessible afterwards (i.e. its oplog can be queried using the persisted mode).
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn create_oplog_entry_persists_durable_agent_mode(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .with_agent_config(
            "CounterAgent",
            vec![AgentConfigEntryDto {
                path: vec!["var1".to_string()],
                value: serde_json::Value::String("value1".to_string()).into(),
            }],
        )
        .store()
        .await?;
    let agent_id = agent_id!("CounterAgent", "persistence-test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Drive the agent so that the Create entry has been written
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let create_entry = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Create(params) => Some(params),
            _ => None,
        })
        .expect("Expected a Create entry at the start of the oplog");

    assert_eq!(create_entry.agent_mode, AgentMode::Durable);

    // The worker remains queryable using its persisted mode.
    executor.check_oplog_is_queryable(&worker_id).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.agent_id, worker_id);

    Ok(())
}

/// Verifies that `AgentMode` is persisted in the `Create` oplog entry of an Ephemeral agent.
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn create_oplog_entry_persists_ephemeral_agent_mode(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let logical_agent_id = agent_id!("EphemeralEchoAgent", "persistence-test");
    let idempotency_key = IdempotencyKey::fresh();
    let agent_id = logical_agent_id
        .with_ephemeral_invocation_phantom(&idempotency_key)
        .map_err(anyhow::Error::msg)?;
    let worker_id = AgentId::from_agent_id(component.id, &agent_id).map_err(anyhow::Error::msg)?;

    // Trigger an invocation so the worker has actually been instantiated and Create persisted.
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let create_entry = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Create(params) => Some(params),
            _ => None,
        })
        .expect("Expected a Create entry at the start of the oplog");

    assert_eq!(create_entry.agent_mode, AgentMode::Ephemeral);
    Ok(())
}
