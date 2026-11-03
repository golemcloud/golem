// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_service_base::model::agent_secret::AgentSecret;
use test_r::{test, timeout};

// Each JavaScript SDK template routes `node:child_process` to the tools bound to the calling
// component: the callers run the Rust provider's `streaming` tool synchronously and through a
// callback, and an unknown command is a usage error.
#[test]
#[tracing::instrument]
#[timeout("10m")]
async fn sdk_templates_dispatch_child_processes_to_bound_tools(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] rust_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_caller")] typescript_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_effect_caller")] effect_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_scala")] scala: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    // The Effect caller declares this secret in its agent config.
    environment_state.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["secret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String("child-process-secret".to_string())),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider = executor
        .component_dep(&context.default_environment_id, rust_provider)
        .store()
        .await?;
    let provider_tools = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", rust_provider.wasm_name)),
        false,
        true,
    )
    .await?
    .tools;

    for (language, component, agent_type) in [
        ("typescript", typescript_caller, "TsToolStreamingCaller"),
        ("effect", effect_caller, "EffectToolStreamingCaller"),
        ("scala", scala, "ScalaToolStreamingCaller"),
    ] {
        let caller = executor
            .component_dep(&context.default_environment_id, component)
            .store()
            .await?;
        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller.id,
            caller.revision,
            Some(deployment_state(
                context.account_id,
                provider.id,
                provider.revision,
                "golem-it:tool-streaming-rust-provider",
                agent_type,
                provider_tools.clone(),
            )),
        );

        let agent_id = agent_id!(agent_type, format!("child-process-{language}"));
        let observation: String = executor
            .invoke_and_await_agent(
                &caller,
                &agent_id,
                "child_process_tool_observation",
                data_value!(),
            )
            .await?
            .into_typed()?;
        assert_eq!(
            observation, "sync=0:000001010202;async=000001010202;usage=2",
            "{language} caller"
        );

        // The adapter is shared, so its edge cases run once, from the TypeScript caller.
        if language == "typescript" {
            let edge_cases: String = executor
                .invoke_and_await_agent(
                    &caller,
                    &agent_id,
                    "child_process_tool_edge_cases",
                    data_value!(),
                )
                .await?
                .into_typed()?;
            assert_eq!(
                edge_cases,
                "echo=0:ping;overflow=ERR_CHILD_PROCESS_STDIO_MAXBUFFER;\
                 abort=error:AbortError,exit:null:SIGTERM;disabled=error"
            );
        }
    }

    Ok(())
}
