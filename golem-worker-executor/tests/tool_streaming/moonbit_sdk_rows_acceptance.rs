// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use test_r::{test, timeout};

#[derive(Debug, FromSchema, PartialEq)]
struct MoonBitMatrixObservation {
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

#[derive(Debug, FromSchema)]
struct MoonBitMatrixReflectionObservation {
    generated_provider: String,
    generated_principal: String,
    generated_owner_agent_id: String,
    reflected_json: String,
    reflected_error_name: String,
    reflected_error_field: String,
    reflected_error_reason: String,
    reflected_error_retryable: bool,
}

#[derive(Debug, FromSchema)]
struct MoonBitResourceObservation {
    secret_first_provider: String,
    secret_second_provider: String,
    secret_first_revealed: bool,
    secret_second_revealed: bool,
    secret_principal: String,
    secret_owner_agent_id: String,
    quota_provider: String,
    quota_reserved: bool,
    quota_returned_usable: bool,
    quota_original_consumed: bool,
    quota_principal: String,
    quota_owner_agent_id: String,
    permission_supported: bool,
    permission_provider: String,
    permission_same_identity: bool,
    permission_original_consumed: bool,
    permission_principal: String,
    permission_owner_agent_id: String,
    typed_values: Vec<u32>,
}

#[derive(Debug, FromSchema)]
struct MoonBitLifecycleObservation {
    bytes: Vec<u8>,
    stream_terminal: String,
    result_terminal: String,
}

fn moonbit_principal(subject: &str) -> Principal {
    Principal::Oidc(OidcPrincipal {
        sub: subject.to_string(),
        issuer: "https://moonbit-sdk-acceptance.test".to_string(),
        email: None,
        name: None,
        email_verified: None,
        given_name: None,
        family_name: None,
        picture: None,
        preferred_username: None,
        claims: "{}".to_string(),
    })
}

async fn assert_moonbit_owner_resources_clean(
    executor: &TestWorkerExecutor,
    owned_agent_id: &OwnedAgentId,
    scenario: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(owned_agent_id)
                .await
                .is_none_or(|active| {
                    active.tool_operations.operations.is_empty()
                        && active.lane.holder.is_none()
                        && active.lane.active_invocation_count == 0
                        && active.slots.iter().all(|slot| slot.invocations.is_empty())
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {scenario} cleanup"))?;
    Ok(())
}

async fn assert_moonbit_owner_clean(
    executor: &TestWorkerExecutor,
    owned_agent_id: &OwnedAgentId,
    worker_id: &golem_common::model::AgentId,
    scenario: &str,
) -> anyhow::Result<()> {
    assert_moonbit_owner_resources_clean(executor, owned_agent_id, scenario).await?;
    executor
        .wait_for_status(
            worker_id,
            AgentStatus::Idle,
            std::time::Duration::from_secs(10),
        )
        .await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn moonbit_generated_and_definition_owned_proxy_match_nested_error_principal_and_owner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_service_base::model::agent_secret::AgentSecret;

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
    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let tools = extract_component_metadata(&path, false, true).await?.tools;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-examples",
            "MoonBitToolStreamingCaller",
            tools,
        )),
    );

    let agent_id = agent_id!("MoonBitToolStreamingCaller", "moonbit-sdk-core");
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let owner_agent_id = agent_id.to_string();

    for subject in ["proxy-principal-a", "proxy-principal-b"] {
        let observation: MoonBitMatrixObservation = executor
            .invoke_and_await_agent_as_principal(
                &stored_component,
                &agent_id,
                moonbit_principal(subject),
                "matrix_core_observation",
                data_value!(),
            )
            .await?
            .into_typed()?;
        assert_eq!(observation.provider, "moonbit");
        assert_eq!(observation.command, "artifact/inspect");
        assert_eq!(observation.normalized_source, "MATRIX.SAMPLE");
        assert_eq!(observation.weighted_size, 108);
        assert_eq!(observation.label_summary, "south|east|north");
        assert_eq!(observation.principal, format!("oidc:{subject}"));
        assert_eq!(observation.owner_agent_id, owner_agent_id);
        assert_eq!(observation.error_field, "request.source");
        assert_eq!(observation.error_reason, "unsupported source");
        assert!(!observation.error_retryable);
        let definition_observation: MoonBitMatrixObservation = executor
            .invoke_and_await_agent_as_principal(
                &stored_component,
                &agent_id,
                moonbit_principal(subject),
                "matrix_core_definition_observation",
                data_value!(),
            )
            .await?
            .into_typed()?;
        assert_eq!(definition_observation, observation);
        assert_moonbit_owner_clean(
            &executor,
            &owned_agent_id,
            &worker_id,
            "MoonBit generated matrix-core",
        )
        .await?;
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn moonbit_reflected_and_generated_runtime_match_success_error_principal_and_owner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
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
    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let tools = extract_component_metadata(&path, false, true).await?.tools;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-examples",
            "MoonBitToolStreamingCaller",
            tools,
        )),
    );

    let agent_id = agent_id!("MoonBitToolStreamingCaller", "moonbit-reflection");
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let observation: MoonBitMatrixReflectionObservation = executor
        .invoke_and_await_agent_as_principal(
            &stored_component,
            &agent_id,
            moonbit_principal("reflection-principal"),
            "matrix_core_reflection_observation",
            data_value!(),
        )
        .await?
        .into_typed()?;

    assert_eq!(observation.generated_provider, "moonbit");
    assert_eq!(observation.generated_principal, "oidc:reflection-principal");
    assert_eq!(observation.generated_owner_agent_id, agent_id.to_string());
    let reflected: serde_json::Value = serde_json::from_str(&observation.reflected_json)?;
    assert_eq!(reflected["provider"], "moonbit");
    assert_eq!(reflected["command"], "artifact/inspect");
    assert_eq!(reflected["normalizedSource"], "MATRIX.SAMPLE");
    assert_eq!(reflected["weightedSize"], "108");
    assert_eq!(reflected["labelSummary"], "south|east|north");
    assert_eq!(reflected["principal"], observation.generated_principal);
    assert_eq!(
        reflected["ownerAgentId"],
        observation.generated_owner_agent_id
    );
    assert_eq!(observation.reflected_error_name, "rejected");
    assert_eq!(observation.reflected_error_field, "request.source");
    assert_eq!(observation.reflected_error_reason, "unsupported source");
    assert!(!observation.reflected_error_retryable);
    assert_moonbit_owner_clean(
        &executor,
        &owned_agent_id,
        &worker_id,
        "MoonBit reflected matrix-core",
    )
    .await?;

    Ok(())
}

async fn check_moonbit_resource_observation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
    scenario: &str,
    method: &str,
    check: impl FnOnce(&MoonBitResourceObservation, &str),
) -> anyhow::Result<()> {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_service_base::model::agent_secret::AgentSecret;

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
    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let tools = extract_component_metadata(&path, false, true).await?.tools;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-examples",
            "MoonBitResourceToolStreamingCaller",
            tools,
        )),
    );

    let agent_id = agent_id!(
        "MoonBitResourceToolStreamingCaller",
        "moonbit-sdk-resources"
    );
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let owner_agent_id = agent_id.to_string();
    let observation: MoonBitResourceObservation = executor
        .invoke_and_await_agent_as_principal(
            &stored_component,
            &agent_id,
            moonbit_principal("resource-principal"),
            method,
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert!(!observation.permission_supported);
    assert_eq!(observation.permission_provider, "");
    assert!(!observation.permission_same_identity);
    assert!(!observation.permission_original_consumed);
    assert_eq!(observation.permission_principal, "");
    assert_eq!(observation.permission_owner_agent_id, "");
    check(&observation, &owner_agent_id);
    assert_moonbit_owner_clean(&executor, &owned_agent_id, &worker_id, scenario).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn moonbit_resource_secret_returned_handle_runtime(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_moonbit_resource_observation(
        last_unique_id,
        deps,
        component,
        "MoonBit secret",
        "matrix_secret_observation",
        |o, owner| {
            assert_eq!(o.secret_first_provider, "moonbit");
            assert_eq!(o.secret_second_provider, "moonbit");
            assert!(o.secret_first_revealed);
            assert!(o.secret_second_revealed);
            assert_eq!(o.secret_principal, "oidc:resource-principal");
            assert_eq!(o.secret_owner_agent_id, owner);
        },
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn moonbit_resource_quota_returned_handle_runtime(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_moonbit_resource_observation(
        last_unique_id,
        deps,
        component,
        "MoonBit quota",
        "matrix_quota_observation",
        |o, owner| {
            assert_eq!(o.quota_provider, "moonbit");
            assert!(o.quota_reserved);
            assert!(o.quota_returned_usable);
            assert!(o.quota_original_consumed);
            assert_eq!(o.quota_principal, "oidc:resource-principal");
            assert_eq!(o.quota_owner_agent_id, owner);
        },
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn moonbit_resource_typed_stream_runtime(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_moonbit_resource_observation(
        last_unique_id,
        deps,
        component,
        "MoonBit typed stream",
        "matrix_typed_stream_observation",
        |o, _| {
            assert_eq!(o.typed_values, [7, 16, 28]);
        },
    )
    .await
}

enum MoonBitExistingTerminalCase {
    PartialSuccess,
    PartialDeclaredError,
    ExplicitFailure,
    DualOutput,
}

async fn check_moonbit_existing_terminal_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
    case: MoonBitExistingTerminalCase,
) -> anyhow::Result<()> {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_service_base::model::agent_secret::AgentSecret;

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
    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let tools = extract_component_metadata(&path, false, true).await?.tools;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-examples",
            "MoonBitToolStreamingCaller",
            tools,
        )),
    );

    let agent_id = agent_id!("MoonBitToolStreamingCaller", "moonbit-sdk-terminals");
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let scenario = match case {
        MoonBitExistingTerminalCase::PartialSuccess => {
            let payload = b"moonbit-terminal-success".to_vec();
            let payload_value = TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions::default())),
                SchemaValue::Binary(BinaryValuePayload {
                    bytes: payload.clone(),
                    mime_type: None,
                }),
            );
            let bytes_read: u64 = executor
                .invoke_and_await_agent(
                    &stored_component,
                    &agent_id,
                    "marker_before_eof",
                    build_input_record(vec![payload_value])?,
                )
                .await?
                .into_typed()?;
            assert_eq!(bytes_read, payload.len() as u64);
            "MoonBit partial-byte success"
        }
        MoonBitExistingTerminalCase::PartialDeclaredError => {
            let declared: String = executor
                .invoke_and_await_agent(
                    &stored_component,
                    &agent_id,
                    "declared_error_completion",
                    data_value!(),
                )
                .await?
                .into_typed()?;
            assert_eq!(declared, "finished:declared:expected");
            "MoonBit partial-byte declared error"
        }
        MoonBitExistingTerminalCase::ExplicitFailure => {
            let explicit_failure: String = executor
                .invoke_and_await_agent(
                    &stored_component,
                    &agent_id,
                    "explicit_stdout_failure",
                    data_value!(),
                )
                .await?
                .into_typed()?;
            assert_eq!(explicit_failure, "resource-exhausted:ok");
            "MoonBit explicit stdout failure"
        }
        MoonBitExistingTerminalCase::DualOutput => {
            let dual: MoonBitDualOutputEvidence = executor
                .invoke_and_await_agent(
                    &stored_component,
                    &agent_id,
                    "dual_output_declared_error",
                    data_value!(),
                )
                .await?
                .into_typed()?;
            assert_eq!(dual.stdout, b"moon-out:\x00\xff");
            assert_eq!(dual.stderr, b"moon-err:\x80!");
            assert_eq!(dual.result, "declared:dual-expected");
            "MoonBit dual-output declared error"
        }
    };
    assert_moonbit_owner_clean(&executor, &owned_agent_id, &worker_id, scenario).await?;

    Ok(())
}

macro_rules! moonbit_existing_terminal_test {
    ($name:ident, $case:expr) => {
        #[test]
        #[tracing::instrument]
        #[timeout("90s")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            check_moonbit_existing_terminal_case(last_unique_id, deps, component, $case).await
        }
    };
}

moonbit_existing_terminal_test!(
    moonbit_terminal_partial_success_runtime,
    MoonBitExistingTerminalCase::PartialSuccess
);
moonbit_existing_terminal_test!(
    moonbit_terminal_partial_declared_error_runtime,
    MoonBitExistingTerminalCase::PartialDeclaredError
);
moonbit_existing_terminal_test!(
    moonbit_terminal_explicit_failure_runtime,
    MoonBitExistingTerminalCase::ExplicitFailure
);
moonbit_existing_terminal_test!(
    moonbit_terminal_dual_output_runtime,
    MoonBitExistingTerminalCase::DualOutput
);

async fn check_moonbit_lifecycle_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
    method: &str,
    bytes: &[u8],
    stream: &str,
    result: &str,
    expected_invocation_failure: Option<&str>,
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
    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let tools = extract_component_metadata(&path, false, true).await?.tools;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-lifecycle-gol40",
            "MoonBitLifecycleGol40Caller",
            tools,
        )),
    );

    let agent_id = agent_id!("MoonBitLifecycleGol40Caller", "runtime-tuples");
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let response = executor
        .invoke_and_await_agent(&stored_component, &agent_id, method, data_value!())
        .await;
    if let Some(expected) = expected_invocation_failure {
        match response {
            Err(error) => {
                let message = format!("{error:#}");
                assert!(
                    message.contains(expected),
                    "{method} failure did not contain {expected:?}: {message}"
                );
            }
            Ok(_) => anyhow::bail!("{method} unexpectedly returned a lifecycle observation"),
        }
        assert_moonbit_owner_resources_clean(
            &executor,
            &owned_agent_id,
            &format!("MoonBit lifecycle {method}"),
        )
        .await?;
        return Ok(());
    }
    let observation: MoonBitLifecycleObservation = response?.into_typed()?;
    assert_eq!(observation.bytes, bytes, "{method} bytes");
    assert_eq!(observation.stream_terminal, stream, "{method} terminal");
    assert_eq!(observation.result_terminal, result, "{method} result");
    assert_moonbit_owner_clean(
        &executor,
        &owned_agent_id,
        &worker_id,
        &format!("MoonBit lifecycle {method}"),
    )
    .await?;

    Ok(())
}

macro_rules! moonbit_lifecycle_test {
    ($name:ident, $method:literal, $bytes:expr, $stream:literal, $result:literal) => {
        #[test]
        #[tracing::instrument]
        #[timeout("45s")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("tool_streaming_moonbit_lifecycle_gol40")] component: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            check_moonbit_lifecycle_case(
                last_unique_id,
                deps,
                component,
                $method,
                $bytes,
                $stream,
                $result,
                None,
            )
            .await
        }
    };
}

moonbit_lifecycle_test!(
    moonbit_lifecycle_complete_runtime,
    "complete",
    b"complete:\x00\xff",
    "finished",
    "ok:41"
);
moonbit_lifecycle_test!(
    moonbit_lifecycle_declared_error_runtime,
    "declared",
    b"declared:\x80",
    "finished",
    "declared:planned"
);
moonbit_lifecycle_test!(
    moonbit_lifecycle_writer_abandonment_runtime,
    "writer_abandonment",
    b"abandoned:",
    "abandoned",
    "ok:43"
);

#[test]
#[tracing::instrument]
#[timeout("45s")]
async fn moonbit_lifecycle_exception_runtime(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit_lifecycle_gol40")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_moonbit_lifecycle_case(
        last_unique_id,
        deps,
        component,
        "exception",
        b"",
        "",
        "",
        Some("Component trapped"),
    )
    .await
}

moonbit_lifecycle_test!(
    moonbit_lifecycle_explicit_invocation_cancellation_runtime,
    "explicit_cancellation",
    b"",
    "cancelled",
    "cancelled"
);
moonbit_lifecycle_test!(
    moonbit_lifecycle_finish_failure_runtime,
    "finish_failure",
    b"finish:",
    "consumer-cancelled",
    "consumer-cancelled"
);
moonbit_lifecycle_test!(
    moonbit_lifecycle_output_reader_cancellation_runtime,
    "output_reader_cancellation",
    b"",
    "reader-cancelled",
    "ok:53"
);
moonbit_lifecycle_test!(
    moonbit_lifecycle_dropped_observer_runtime,
    "dropped_observer",
    b"complete:\x00\xff",
    "finished",
    "observer-dropped"
);
