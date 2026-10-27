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

//! A tool result is an async guest event. The guest receives it only at a blocked wait, and the
//! oplog records that boundary, so a rebuild releases the result at the same point.

use super::*;
use test_r::test;

/// The caller starts a short tool call, blocks once on a timer and then only yields while it polls
/// the result. A yield is not a blocked wait, so the result must not reach the guest while it
/// yields, and a rebuild must replay the call with the result at the same guest boundary.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn yielding_guest_receives_tool_result_only_at_a_blocked_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let agent = agent_id!("ToolStreamingCaller", "yielding-tool-result");
    let worker_id = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;

    let observed_while_yielding: bool = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "yielding_tool_result_observation",
            data_value!(100_000u32),
        )
        .await?
        .into_typed()?;
    assert!(
        !observed_while_yielding,
        "the tool result reached a guest that only yielded"
    );
    let recorded = executor.stored_oplog(&worker_id).await;

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed: String = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the rebuild did not replay the tool result"))??
    .into_typed()?;
    assert_eq!(replayed, "replayed");
    let rebuilt = executor.stored_oplog(&worker_id).await;
    assert_eq!(
        &rebuilt[..recorded.len()],
        &recorded[..],
        "the rebuild changed the recorded history"
    );
    Ok(())
}
