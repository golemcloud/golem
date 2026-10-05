// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
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

#[derive(Debug, FromSchema)]
struct MatrixResourceObservation {
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

struct MatrixFixture<'a> {
    language: &'static str,
    component: &'a PrecompiledComponent,
    package: &'static str,
    agent_type: &'static str,
    method: &'static str,
}

struct MatrixResourceCaller<'a> {
    language: &'static str,
    component: &'a PrecompiledComponent,
    agent_type: &'static str,
    method: &'static str,
    provider_index: usize,
    supports_secret: bool,
    supports_quota: bool,
    supports_permission: bool,
    supports_typed_stream: bool,
}

fn oidc_matrix_principal(subject: &str) -> Principal {
    Principal::Oidc(OidcPrincipal {
        sub: subject.to_string(),
        issuer: "https://matrix-conformance.test".to_string(),
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

fn assert_matrix_observation(
    observation: &MatrixObservation,
    caller: &str,
    provider: &str,
    principal: &str,
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
    assert_eq!(observation.principal, principal, "{path}.principal");
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

fn assert_matrix_resource_observation(
    observation: &MatrixResourceObservation,
    caller: &MatrixResourceCaller<'_>,
    provider: &MatrixFixture<'_>,
    principal: &str,
    owner_agent_id: &str,
) {
    let path = format!(
        "resource-matrix[{}->{}]",
        caller.language, provider.language
    );
    if caller.supports_secret {
        assert_eq!(
            observation.secret_first_provider, provider.language,
            "{path}.secretFirstProvider"
        );
        assert_eq!(
            observation.secret_second_provider, provider.language,
            "{path}.secretSecondProvider"
        );
        assert!(
            observation.secret_first_revealed,
            "{path}.secretFirstRevealed"
        );
        assert!(
            observation.secret_second_revealed,
            "{path}.secretSecondRevealed"
        );
        assert_eq!(
            observation.secret_principal, principal,
            "{path}.secretPrincipal"
        );
        assert_eq!(
            observation.secret_owner_agent_id, owner_agent_id,
            "{path}.secretOwnerAgentId"
        );
    } else {
        assert_eq!(
            observation.secret_first_provider, "",
            "{path}.secretFirstProvider"
        );
        assert_eq!(
            observation.secret_second_provider, "",
            "{path}.secretSecondProvider"
        );
        assert!(
            !observation.secret_first_revealed,
            "{path}.secretFirstRevealed"
        );
        assert!(
            !observation.secret_second_revealed,
            "{path}.secretSecondRevealed"
        );
        assert_eq!(observation.secret_principal, "", "{path}.secretPrincipal");
        assert_eq!(
            observation.secret_owner_agent_id, "",
            "{path}.secretOwnerAgentId"
        );
    }

    if caller.supports_quota {
        assert_eq!(
            observation.quota_provider, provider.language,
            "{path}.quotaProvider"
        );
        assert!(observation.quota_reserved, "{path}.quotaReserved");
        assert!(
            observation.quota_returned_usable,
            "{path}.quotaReturnedUsable"
        );
        assert!(
            observation.quota_original_consumed,
            "{path}.quotaOriginalConsumed"
        );
        assert_eq!(
            observation.quota_principal, principal,
            "{path}.quotaPrincipal"
        );
        assert_eq!(
            observation.quota_owner_agent_id, owner_agent_id,
            "{path}.quotaOwnerAgentId"
        );
    } else {
        assert_eq!(observation.quota_provider, "", "{path}.quotaProvider");
        assert!(!observation.quota_reserved, "{path}.quotaReserved");
        assert!(
            !observation.quota_returned_usable,
            "{path}.quotaReturnedUsable"
        );
        assert!(
            !observation.quota_original_consumed,
            "{path}.quotaOriginalConsumed"
        );
        assert_eq!(observation.quota_principal, "", "{path}.quotaPrincipal");
        assert_eq!(
            observation.quota_owner_agent_id, "",
            "{path}.quotaOwnerAgentId"
        );
    }

    assert_eq!(
        observation.permission_supported, caller.supports_permission,
        "{path}.permissionSupported"
    );
    if caller.supports_permission {
        assert_eq!(
            observation.permission_provider, provider.language,
            "{path}.permissionProvider"
        );
        assert!(
            observation.permission_same_identity,
            "{path}.permissionSameIdentity"
        );
        assert!(
            observation.permission_original_consumed,
            "{path}.permissionOriginalConsumed"
        );
        assert_eq!(
            observation.permission_principal, principal,
            "{path}.permissionPrincipal"
        );
        assert_eq!(
            observation.permission_owner_agent_id, owner_agent_id,
            "{path}.permissionOwnerAgentId"
        );
    } else {
        assert_eq!(
            observation.permission_provider, "",
            "{path}.permissionProvider"
        );
        assert!(
            !observation.permission_same_identity,
            "{path}.permissionSameIdentity"
        );
        assert!(
            !observation.permission_original_consumed,
            "{path}.permissionOriginalConsumed"
        );
        assert_eq!(
            observation.permission_principal, "",
            "{path}.permissionPrincipal"
        );
        assert_eq!(
            observation.permission_owner_agent_id, "",
            "{path}.permissionOwnerAgentId"
        );
    }
    if caller.supports_typed_stream {
        assert_eq!(observation.typed_values, [7, 16, 28], "{path}.typedValues");
    } else {
        assert!(observation.typed_values.is_empty(), "{path}.typedValues");
    }
}

async fn assert_matrix_resources_cleaned_up(
    executor: &TestWorkerExecutor,
    owned_agent_id: &OwnedAgentId,
    path: &str,
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
    .map_err(|_| anyhow::anyhow!("timed out waiting for {path} resource cleanup"))?;
    Ok(())
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
                .invoke_and_await_agent_as_principal(
                    caller_component,
                    &agent_id,
                    oidc_matrix_principal("matrix-principal-a"),
                    caller.method,
                    data_value!(),
                )
                .await?
                .into_typed()?;
            assert_matrix_observation(
                &observation,
                caller.language,
                provider.language,
                "oidc:matrix-principal-a",
                &owner_agent_id,
            );

            if provider_index == (caller_index + 1) % providers.len() {
                let second_observation: MatrixObservation = executor
                    .invoke_and_await_agent_as_principal(
                        caller_component,
                        &agent_id,
                        oidc_matrix_principal("matrix-principal-b"),
                        caller.method,
                        data_value!(),
                    )
                    .await?
                    .into_typed()?;
                assert_matrix_observation(
                    &second_observation,
                    caller.language,
                    provider.language,
                    "oidc:matrix-principal-b",
                    &owner_agent_id,
                );
                assert_ne!(
                    observation.principal, second_observation.principal,
                    "matrix[{}->{}].principal must follow the runtime caller",
                    caller.language, provider.language
                );
                assert_eq!(
                    observation.owner_agent_id, second_observation.owner_agent_id,
                    "matrix[{}->{}].ownerAgentId must not follow the runtime principal",
                    caller.language, provider.language
                );
            }
        }
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("10m")]
async fn generated_clients_transfer_supported_resources_and_typed_streams(
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
    ];
    let callers = [
        MatrixResourceCaller {
            language: "rust",
            component: rust_caller,
            agent_type: "RustResourceToolStreamingCaller",
            method: "matrix_resource_observation",
            provider_index: 3,
            supports_secret: true,
            supports_quota: true,
            supports_permission: false,
            supports_typed_stream: true,
        },
        MatrixResourceCaller {
            language: "typescript",
            component: typescript_caller,
            agent_type: "TsResourceToolStreamingCaller",
            method: "matrix_resource_observation",
            provider_index: 1,
            supports_secret: true,
            supports_quota: true,
            supports_permission: false,
            supports_typed_stream: false,
        },
        MatrixResourceCaller {
            language: "scala",
            component: scala,
            agent_type: "ScalaResourceToolStreamingCaller",
            method: "matrix_typed_stream_observation",
            provider_index: 2,
            supports_secret: false,
            supports_quota: false,
            supports_permission: false,
            supports_typed_stream: true,
        },
        MatrixResourceCaller {
            language: "moonbit",
            component: moonbit,
            agent_type: "MoonBitResourceToolStreamingCaller",
            method: "matrix_resource_observation",
            provider_index: 3,
            supports_secret: false,
            supports_quota: false,
            supports_permission: false,
            supports_typed_stream: false,
        },
        MatrixResourceCaller {
            language: "effect",
            component: effect_caller,
            agent_type: "EffectToolStreamingCaller",
            method: "matrix_resource_observation",
            provider_index: 4,
            supports_secret: false,
            supports_quota: false,
            supports_permission: false,
            supports_typed_stream: true,
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

    for caller in &callers {
        let caller_component = executor
            .component_dep(&context.default_environment_id, caller.component)
            .store()
            .await?;
        let provider = &providers[caller.provider_index];
        let provider_component = &stored_providers[caller.provider_index];
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
                provider_tools[caller.provider_index].clone(),
            )),
        );

        let agent_id = agent_id!(caller.agent_type, format!("resource-{}", caller.language));
        let worker_id = executor
            .start_agent(&caller_component.id, agent_id.clone())
            .await?;
        let owner_agent_id = agent_id.to_string();
        let observation: MatrixResourceObservation = executor
            .invoke_and_await_agent_as_principal(
                &caller_component,
                &agent_id,
                oidc_matrix_principal("matrix-resource-principal"),
                caller.method,
                data_value!(),
            )
            .await?
            .into_typed()?;
        assert_matrix_resource_observation(
            &observation,
            caller,
            provider,
            "oidc:matrix-resource-principal",
            &owner_agent_id,
        );
        assert_matrix_resources_cleaned_up(
            &executor,
            &OwnedAgentId::new(context.default_environment_id, &worker_id),
            &format!(
                "resource-matrix[{}->{}]",
                caller.language, provider.language
            ),
        )
        .await?;
    }

    Ok(())
}
