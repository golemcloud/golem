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

use golem_api_grpc::proto::golem::worker::InvocationStart;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::sqlite::SqliteSchedulerStorage;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
    scheduler_sqlite_storage_config, start,
};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
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
#[timeout("60s")]
#[tracing::instrument]
async fn streaming_schedule_is_rejected_without_creating_or_queueing_a_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let scheduler =
        SqliteSchedulerStorage::configured(&scheduler_sqlite_storage_config(deps, &context))
            .await
            .map_err(anyhow::Error::msg)?;
    let agent_id = agent_id!("StreamingRpcTarget", "rejected-schedule");
    let worker_id = AgentId::from_agent_id(component.id, &agent_id).map_err(anyhow::Error::msg)?;
    let (_, input) = data_value!(vec![1_u32, 2, 3]).into_parts();
    let input: golem_schema::proto::golem::schema::SchemaValue =
        input.try_into().map_err(anyhow::Error::msg)?;
    let component_id = component.id.to_string();
    let blobs_before = files_below(&deps.blob_storage_root())?
        .into_iter()
        .filter(|path| path.to_string_lossy().contains(&component_id))
        .collect::<HashSet<_>>();

    for schedule_at in [
        None,
        Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp() + 1,
            nanos: 0,
        }),
    ] {
        let error = executor
            .invoke_agent_session(InvocationStart {
                agent_id: Some(worker_id.clone().into()),
                method_name: Some("produce".to_string()),
                input: Some(input.clone()),
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
                schedule_at,
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                component_owner_account_id: Some(component.account_id.into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(executor.auth_ctx().into()),
                context: None,
                principal: None,
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32,
                config: Vec::new(),
                attempt_id: None,
                expected_callee_fingerprint: None,
                durable_input_mappings: Vec::new(),
                scope_card: None,
                origin_invocation: None,
                external_tool: None,
            })
            .await
            .expect_err("scheduled streaming invocation must be rejected");
        assert!(
            error
                .to_string()
                .contains("require an attached Await invocation session"),
            "unexpected error: {error}"
        );
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(executor.get_worker_metadata_opt(&worker_id).await?, None);
    let assignment =
        golem_common::model::ShardAssignment::unexpiring(1, [golem_common::model::ShardId::new(0)]);
    assert_eq!(
        scheduler
            .count_due(chrono::Utc::now() + chrono::Duration::days(1), &assignment)
            .await?,
        0,
        "rejection must not create an immediate or delayed scheduled action"
    );
    assert_eq!(
        files_below(&deps.blob_storage_root())?
            .into_iter()
            .filter(|path| path.to_string_lossy().contains(&component_id))
            .collect::<HashSet<_>>(),
        blobs_before,
        "rejection must not upload an invocation or result payload"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn invocation_classification_uses_the_existing_workers_component_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let streaming_component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let streaming_agent_id = agent_id!("StreamingRpcTarget", "pinned-streaming-revision");
    let streaming_worker_id = executor
        .start_agent(&streaming_component.id, streaming_agent_id.clone())
        .await?;
    executor
        .update_component(&streaming_component.id, &agent_rpc.wasm_name)
        .await?;

    let (_, input) = data_value!(vec![1_u32, 2, 3]).into_parts();
    let error = executor
        .invoke_agent_session(InvocationStart {
            agent_id: Some(streaming_worker_id.clone().into()),
            method_name: Some("produce".to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: None,
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            component_owner_account_id: Some(streaming_component.account_id.into()),
            environment_id: Some(streaming_component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
        })
        .await
        .expect_err("the old streaming schema must still reject scheduling");
    assert!(
        error
            .to_string()
            .contains("require an attached Await invocation session"),
        "unexpected error: {error}"
    );
    let streaming_metadata = executor.get_worker_metadata(&streaming_worker_id).await?;
    assert_eq!(
        streaming_metadata.component_revision,
        ComponentRevision::INITIAL
    );
    assert_eq!(streaming_metadata.pending_invocation_count, 0);

    let stream_free_component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let stream_free_agent_id = agent_id!("TestAgent", "pinned-stream-free-revision");
    let stream_free_worker_id = executor
        .start_agent(&stream_free_component.id, stream_free_agent_id.clone())
        .await?;
    executor
        .update_component(&stream_free_component.id, &agent_rpc_rust.wasm_name)
        .await?;

    let output = executor
        .invoke_and_await_agent(
            &stream_free_component,
            &stream_free_agent_id,
            "run",
            data_value!(0_f64),
        )
        .await?;
    assert_eq!(
        output.into_return_value(),
        Some(SchemaValue::List {
            elements: Vec::new()
        })
    );
    let stream_free_metadata = executor.get_worker_metadata(&stream_free_worker_id).await?;
    assert_eq!(
        stream_free_metadata.component_revision,
        ComponentRevision::INITIAL
    );
    Ok(())
}

fn files_below(root: &Path) -> anyhow::Result<HashSet<PathBuf>> {
    fn visit(root: &Path, current: &Path, files: &mut HashSet<PathBuf>) -> anyhow::Result<()> {
        if !current.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(current)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(root, &path, files)?;
            } else {
                files.insert(path.strip_prefix(root)?.to_path_buf());
            }
        }
        Ok(())
    }

    let mut files = HashSet::new();
    visit(root, root, &mut files)?;
    Ok(files)
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
        .invoke_agent_session(InvocationStart {
            agent_id: Some(worker_id.into()),
            method_name: Some("changeAndGet".to_string()),
            input: Some(
                SchemaValue::Record { fields: vec![] }
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            ),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
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

    executor
        .invoke_agent_session(InvocationStart {
            agent_id: Some(worker_id.clone().into()),
            method_name: None,
            input: None,
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
        })
        .await?;
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
        .invoke_agent_session(InvocationStart {
            agent_id: Some(worker_id.clone().into()),
            method_name: Some("changeAndGet".to_string()),
            input: Some(
                SchemaValue::Record { fields: vec![] }
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            ),
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
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
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

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn archived_ephemeral_agent_remains_observable_and_can_be_deleted(
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
    let logical_agent_id = agent_id!("EphemeralEchoAgent", "archived-delete");

    let result = executor
        .invoke_and_await_agent(&component, &logical_agent_id, "changeAndGet", data_value!())
        .await?;
    let final_agent_id = result.agent_id().clone();
    assert_eq!(result.into_typed::<String>()?, "archived-delete!");
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &final_agent_id);

    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while executor.worker_is_cached(&owned_agent_id).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    let metadata = executor.get_worker_metadata(&final_agent_id).await?;
    assert_eq!(metadata.agent_id, final_agent_id);
    assert!(
        !executor
            .get_oplog(&final_agent_id, OplogIndex::INITIAL)
            .await?
            .is_empty()
    );

    executor.delete_worker(&final_agent_id).await?;
    assert_eq!(
        executor.get_worker_metadata_opt(&final_agent_id).await?,
        None
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn fork_publication_retry_preserves_independent_state_across_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::durable_stream::StreamSessionRecord;
    use golem_common::schema::FromSchema;

    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, true).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let source = agent_id!("RpcCounter", "fork-retry");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "inc_by", data_value!(5u64))
        .await?;
    let prefix = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
    let cut = prefix.last().unwrap().oplog_index;
    let target = golem_common::phantom_agent_id!("RpcCounter", uuid::Uuid::new_v4(), "fork-retry");
    let target_id = AgentId::from_agent_id(component.id, &target).map_err(anyhow::Error::msg)?;
    let error = executor
        .fork_worker(&source_id, &target.to_string(), cut)
        .await
        .expect_err("injected lost response");
    assert!(
        error.to_string().contains("lost fork resume response"),
        "{error}"
    );
    executor
        .fork_worker(&source_id, &target.to_string(), cut)
        .await?;
    executor
        .invoke_and_await_agent(&component, &source, "inc_by", data_value!(11u64))
        .await?;
    executor
        .invoke_and_await_agent(&component, &target, "inc_by", data_value!(2u64))
        .await?;
    // Retrying after the target has run must not overwrite it with the old prefix.
    executor
        .fork_worker(&source_id, &target.to_string(), cut)
        .await?;
    assert!(
        executor
            .fork_worker(&source_id, &target.to_string(), cut.previous())
            .await
            .is_err()
    );
    drop(executor);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    executor
        .fork_worker(&source_id, &target.to_string(), cut)
        .await?;
    let actual = executor
        .invoke_and_await_agent(&component, &target, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(actual, 7);
    let actual = executor
        .invoke_and_await_agent(&component, &source, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(actual, 16);
    let fork_history = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
    let markers = fork_history
        .iter()
        .filter_map(|entry| {
            let PublicOplogEntry::StreamSession(record) = &entry.entry else {
                return None;
            };
            let StreamSessionRecord::ForkCut(cut) =
                StreamSessionRecord::from_value(record.record.value()).ok()?
            else {
                return None;
            };
            Some((entry.oplog_index, cut))
        })
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].0, cut.next());
    assert_eq!(markers[0].1.cut_index, cut);
    assert_eq!(
        markers[0].1.creation_fingerprint,
        executor.get_worker_metadata(&target_id).await?.fingerprint
    );
    Ok(())
}

#[test]
#[timeout("180s")]
async fn pending_payload_reuse_preserves_counter_effects_across_restart_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogEntry, OplogPayload};
    use golem_service_base::error::worker_executor::InterruptKind;
    use golem_worker_executor::services::HasWasmtimeEngine;
    use golem_worker_executor_test_utils::{TestExecutorOverrides, start_with_overrides};
    use std::sync::Arc;
    use std::time::Duration;

    for max_payload_size in [1, 1_000_000] {
        let context = TestContext::new(last_unique_id);
        let overrides = TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.oplog.max_payload_size = max_payload_size;
            })),
            ..Default::default()
        };
        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        let agent = agent_id!("RpcCounter", format!("payload-reuse-{max_payload_size}"));
        let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            0
        );
        let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
        let initial_loads = executor.instance_load_count(&worker_id);
        let first_key = IdempotencyKey::fresh();
        let mut started = executor.gate_next_invocation_started(&owned).await?;
        let first = {
            let executor = executor.clone();
            let component = component.clone();
            let agent = agent.clone();
            let key = first_key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent,
                        &key,
                        "inc_by",
                        data_value!(7u64),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(20), started.entered()).await?;
        let history = executor.stored_oplog(&worker_id).await;
        assert!(history.iter().any(|entry| matches!(entry,
            OplogEntry::PendingAgentInvocation { idempotency_key, .. } if *idempotency_key == first_key)));
        assert!(!history.iter().any(|entry| matches!(entry,
            OplogEntry::AgentInvocationStarted { idempotency_key, .. } if *idempotency_key == first_key)));
        executor
            .interrupt_loaded_worker(&worker_id, InterruptKind::Restart)
            .await?;
        executor
            .active_agent(&owned)
            .await
            .unwrap()
            .primary()
            .engine()
            .increment_epoch();
        started.release();
        tokio::time::timeout(Duration::from_secs(60), first).await???;
        assert!(executor.instance_load_count(&worker_id) > initial_loads);

        let second_key = IdempotencyKey::fresh();
        let before_completion_restart = executor.instance_load_count(&worker_id);
        let mut completion = executor.gate_next_agent_invocation_success(&worker_id);
        let second = {
            let executor = executor.clone();
            let component = component.clone();
            let agent = agent.clone();
            let key = second_key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent,
                        &key,
                        "inc_by",
                        data_value!(11u64),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(20), completion.entered()).await?;
        let history = executor.stored_oplog(&worker_id).await;
        assert!(history.iter().any(|entry| matches!(entry,
            OplogEntry::AgentInvocationStarted { idempotency_key, .. } if *idempotency_key == second_key)));
        assert!(!history.iter().rev().take_while(|entry| !matches!(entry,
            OplogEntry::AgentInvocationStarted { idempotency_key, .. } if *idempotency_key == second_key))
            .any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. })));
        completion.abort_as_restart();
        tokio::time::timeout(Duration::from_secs(60), second).await???;
        assert!(executor.instance_load_count(&worker_id) > before_completion_restart);
        let output_key = IdempotencyKey::fresh();
        let expected_output = "x".repeat(10_000);
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    &output_key,
                    "inc_and_return_text",
                    data_value!(10_000u32),
                )
                .await?
                .into_typed::<String>()?,
            expected_output
        );
        drop(executor);

        let executor = start_with_overrides(deps, &context, overrides).await?;
        for (key, amount) in [(&first_key, 7u64), (&second_key, 11u64)] {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    key,
                    "inc_by",
                    data_value!(amount),
                )
                .await?;
        }
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    &output_key,
                    "inc_and_return_text",
                    data_value!(10_000u32),
                )
                .await?
                .into_typed::<String>()?,
            expected_output
        );
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            19
        );
        let history = executor.stored_oplog(&worker_id).await;
        let mut active_key = None;
        let mut completions = HashMap::new();
        for entry in &history {
            match entry {
                OplogEntry::AgentInvocationStarted {
                    idempotency_key, ..
                } => {
                    active_key = Some(idempotency_key);
                }
                OplogEntry::AgentInvocationFinished { .. } => {
                    *completions
                        .entry(active_key.take().expect("Started before Finished"))
                        .or_insert(0usize) += 1;
                }
                _ => {}
            }
        }
        for key in [&first_key, &second_key, &output_key] {
            let pending = history
                .iter()
                .filter_map(|entry| match entry {
                    OplogEntry::PendingAgentInvocation {
                        idempotency_key,
                        payload,
                        ..
                    } if idempotency_key == key => Some(payload),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(pending.len(), 1);
            assert_eq!(
                matches!(pending[0], OplogPayload::External { .. }),
                max_payload_size == 1
            );
            let starts = history
                .iter()
                .filter_map(|entry| match entry {
                    OplogEntry::AgentInvocationStarted {
                        idempotency_key,
                        payload,
                        ..
                    } if idempotency_key == key => Some(payload),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(!starts.is_empty());
            for payload in starts {
                assert_eq!(payload, pending[0]);
            }
            assert_eq!(completions.get(key), Some(&1));
        }
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn pending_payload_reuse_keeps_manual_snapshot_update_distinct_from_method_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent = agent_id!("SnapshotUpdateTest");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let key = IdempotencyKey::fresh();
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "stable_value",
                data_value!()
            )
            .await?
            .into_typed::<u32>()?,
        7
    );
    executor
        .invoke_and_await_agent(&component, &agent, "pre_snapshot_value", data_value!())
        .await?;
    let target = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, target.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, target.revision, Duration::from_secs(30))
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "loaded_snapshot_revision",
                data_value!()
            )
            .await?
            .into_typed::<u32>()?,
        1
    );
    drop(executor);

    let executor = start(deps, &context).await?;
    executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "stable_value", data_value!())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "accumulated_value", data_value!())
            .await?
            .into_typed::<u32>()?,
        11
    );
    executor
        .invoke_and_await_agent(&component, &agent, "stable_value", data_value!())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "accumulated_value", data_value!())
            .await?
            .into_typed::<u32>()?,
        21
    );
    Ok(())
}
