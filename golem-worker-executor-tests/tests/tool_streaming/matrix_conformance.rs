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
use golem_worker_executor_test_utils::callers;
use test_r::{test, timeout};

#[derive(Debug, FromSchema)]
struct MatrixObservation {
    provider: String,
    command: String,
    normalized_source: String,
    weighted_size: i64,
    label_summary: String,
    principal: String,
    owner_agent_id: String,
    error_field: String,
    error_reason: String,
    error_retryable: bool,
}

struct MatrixFixture<'a> {
    language: &'static str,
    component: &'a PrecompiledComponent,
    package: &'static str,
    agent_type: &'static str,
    method: &'static str,
}

fn oidc_matrix_principal(subject: &str) -> golem_common::model::caller::Caller {
    callers::user_with_claims("https://matrix-conformance.test", subject, "{}")
}

fn assert_matrix_observation(
    observation: &MatrixObservation,
    caller: &str,
    provider: &str,
    expected_caller: &str,
    owner_agent_id: &str,
) {
    let path = format!("matrix[{caller}->{provider}]");
    assert_eq!(observation.provider, provider, "{path}.provider");
    assert_eq!(observation.command, "artifact/inspect", "{path}.command");
    assert_eq!(
        observation.normalized_source, "MATRIX.SAMPLE",
        "{path}.normalizedSource"
    );
    assert_eq!(observation.weighted_size, 108, "{path}.weightedSize");
    assert_eq!(
        observation.label_summary, "south|east|north",
        "{path}.labelSummary"
    );
    assert_eq!(observation.principal, expected_caller, "{path}.principal");
    assert_eq!(
        observation.owner_agent_id, owner_agent_id,
        "{path}.ownerAgentId"
    );
    assert_eq!(
        observation.error_field, "request.source",
        "{path}.errorField"
    );
    assert_eq!(
        observation.error_reason, "unsupported source",
        "{path}.errorReason"
    );
    assert!(
        !observation.error_retryable,
        "{path}.errorRetryable: expected false, got true"
    );
}

#[test]
#[tracing::instrument]
#[timeout("10m")]
async fn generated_clients_invoke_every_sdk_provider_and_preserve_runtime_identity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] rust_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] rust_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_provider")] typescript_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_caller")] typescript_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_scala")] scala: &PrecompiledComponent,
    #[tagged_as("tool_streaming_moonbit")] moonbit: &PrecompiledComponent,
    #[tagged_as("tool_streaming_effect_provider")] effect_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_effect_caller")] effect_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_go_provider")] go_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_go_caller")] go_caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    environment_state.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["secret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String("matrix-secret-value".to_string())),
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

    let providers = [
        MatrixFixture {
            language: "rust",
            component: rust_provider,
            package: "golem-it:tool-streaming-rust-provider",
            agent_type: "",
            method: "",
        },
        MatrixFixture {
            language: "typescript",
            component: typescript_provider,
            package: "golem-it:tool-streaming-ts-provider",
            agent_type: "",
            method: "",
        },
        MatrixFixture {
            language: "scala",
            component: scala,
            package: "scala:examples",
            agent_type: "",
            method: "",
        },
        MatrixFixture {
            language: "moonbit",
            component: moonbit,
            package: "golem:moonbit-examples",
            agent_type: "",
            method: "",
        },
        MatrixFixture {
            language: "effect",
            component: effect_provider,
            package: "golem-it:tool-streaming-effect-provider",
            agent_type: "",
            method: "",
        },
        MatrixFixture {
            language: "go",
            component: go_provider,
            package: "golem-it:tool-streaming-go-provider",
            agent_type: "",
            method: "",
        },
    ];
    let callers = [
        MatrixFixture {
            language: "rust",
            component: rust_caller,
            package: "",
            agent_type: "ToolStreamingCaller",
            method: "matrix_core_observation",
        },
        MatrixFixture {
            language: "typescript",
            component: typescript_caller,
            package: "",
            agent_type: "TsToolStreamingCaller",
            method: "matrix_core_observation",
        },
        MatrixFixture {
            language: "scala",
            component: scala,
            package: "",
            agent_type: "ScalaToolStreamingCaller",
            method: "matrix_core_observation",
        },
        MatrixFixture {
            language: "moonbit",
            component: moonbit,
            package: "",
            agent_type: "MoonBitToolStreamingCaller",
            method: "matrix_core_observation",
        },
        MatrixFixture {
            language: "effect",
            component: effect_caller,
            package: "",
            agent_type: "EffectToolStreamingCaller",
            method: "matrix_core_observation",
        },
        MatrixFixture {
            language: "go",
            component: go_caller,
            package: "",
            agent_type: "GoToolStreamingCaller",
            method: "matrix_core_observation",
        },
    ];

    let mut stored_providers = Vec::with_capacity(providers.len());
    let mut provider_tools = Vec::with_capacity(providers.len());
    for provider in &providers {
        stored_providers.push(
            executor
                .component_dep(&context.default_environment_id, provider.component)
                .store()
                .await?,
        );
        let path = deps
            .component_directory
            .join(format!("{}.wasm", provider.component.wasm_name));
        provider_tools.push(extract_component_metadata(&path, false, true).await?.tools);
    }

    let mut stored_callers = Vec::with_capacity(callers.len());
    for caller in &callers {
        stored_callers.push(
            executor
                .component_dep(&context.default_environment_id, caller.component)
                .store()
                .await?,
        );
    }

    for (caller_index, caller) in callers.iter().enumerate() {
        for (provider_index, provider) in providers.iter().enumerate() {
            let caller_component = &stored_callers[caller_index];
            let provider_component = &stored_providers[provider_index];
            environment_state.set_tool_deployment(
                context.default_environment_id,
                caller_component.id,
                caller_component.revision,
                Some(deployment_state(
                    context.account_id,
                    provider_component.id,
                    provider_component.revision,
                    provider.package,
                    caller.agent_type,
                    provider_tools[provider_index].clone(),
                )),
            );

            let agent_id = agent_id!(
                caller.agent_type,
                format!("matrix-{}-{}", caller.language, provider.language)
            );
            let owner_agent_id = agent_id.to_string();
            let observation: MatrixObservation = executor
                .invoke_and_await_agent_as_caller(
                    caller_component,
                    &agent_id,
                    oidc_matrix_principal("matrix-caller-a"),
                    caller.method,
                    data_value!(),
                )
                .await?
                .into_typed()?;
            // The provider sees the user as the original caller and the calling agent as the
            // immediate one.
            let calling_agent = golem_common::model::AgentId {
                component_id: caller_component.id,
                agent_id: owner_agent_id.clone(),
            };
            let calling_agent_text = callers::principal_text(
                &callers::agent_in(
                    calling_agent.clone(),
                    executor
                        .get_worker_metadata(&calling_agent)
                        .await?
                        .fingerprint,
                    context.default_environment_id,
                )
                .immediate,
            );
            assert_matrix_observation(
                &observation,
                caller.language,
                provider.language,
                &format!(
                    "{}|{calling_agent_text}",
                    callers::user_text("https://matrix-conformance.test", "matrix-caller-a")
                ),
                &owner_agent_id,
            );

            if provider_index == (caller_index + 1) % providers.len() {
                let second_observation: MatrixObservation = executor
                    .invoke_and_await_agent_as_caller(
                        caller_component,
                        &agent_id,
                        oidc_matrix_principal("matrix-caller-b"),
                        caller.method,
                        data_value!(),
                    )
                    .await?
                    .into_typed()?;
                assert_matrix_observation(
                    &second_observation,
                    caller.language,
                    provider.language,
                    &format!(
                        "{}|{calling_agent_text}",
                        callers::user_text("https://matrix-conformance.test", "matrix-caller-b")
                    ),
                    &owner_agent_id,
                );
                let original = |principal: &str| principal.split('|').next().unwrap().to_string();
                assert_ne!(
                    original(&observation.principal),
                    original(&second_observation.principal),
                    "matrix[{}->{}].principal must follow the runtime caller",
                    caller.language,
                    provider.language
                );
                assert_eq!(
                    observation.owner_agent_id, second_observation.owner_agent_id,
                    "matrix[{}->{}].ownerAgentId must not follow the runtime caller",
                    caller.language, provider.language
                );
            }
        }
    }

    Ok(())
}

/// A Go caller gives a tool a pipe nobody writes as its stdin, and the tool
/// returns without reading it. The stream closing tells the caller's stdin
/// pump that nobody consumes the pipe, so the pump closes it and releases the
/// call's handles instead of waiting in a Read forever.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn go_caller_closes_the_stdin_a_tool_does_not_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_go_provider")] go_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_go_caller")] go_caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
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
        .component_dep(&context.default_environment_id, go_provider)
        .store()
        .await?;
    let provider_tools = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", go_provider.wasm_name)),
        false,
        true,
    )
    .await?
    .tools;
    let caller = executor
        .component_dep(&context.default_environment_id, go_caller)
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
            "golem-it:tool-streaming-go-provider",
            "GoToolStreamingCaller",
            provider_tools,
        )),
    );

    let agent_id = agent_id!("GoToolStreamingCaller", "stdin-released");
    let released: bool = executor
        .invoke_and_await_agent(&caller, &agent_id, "stdin_released", data_value!())
        .await?
        .into_typed()?;
    assert!(
        released,
        "the caller's stdin pipe stayed open after the tool returned without reading it"
    );
    Ok(())
}
