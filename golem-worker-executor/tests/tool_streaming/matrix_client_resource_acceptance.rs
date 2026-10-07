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

#[derive(Debug, FromSchema)]
struct ResourceObservation {
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

struct Provider<'a> {
    language: &'static str,
    component: &'a PrecompiledComponent,
    package: &'static str,
    supports_secret: bool,
    supports_named_quota: bool,
}

struct Caller<'a> {
    language: &'static str,
    component: &'a PrecompiledComponent,
    agent_type: &'static str,
    supports_named_quota: bool,
}

#[derive(Clone, Copy)]
enum ResourceContract {
    Secret,
    Quota,
    Permission,
    TypedStream,
}

fn root_tool_name(tool: &golem_common::schema::tool::Tool) -> &str {
    tool.commands
        .nodes
        .first()
        .expect("tool definition has a root command")
        .name
        .as_str()
}

fn oidc_principal() -> Principal {
    Principal::Oidc(OidcPrincipal {
        sub: "matrix-resource-acceptance".to_string(),
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

fn push_eq<T: std::fmt::Debug + PartialEq + ?Sized>(
    failures: &mut Vec<String>,
    path: &str,
    actual: &T,
    expected: &T,
) {
    if actual != expected {
        failures.push(format!("{path}: expected {expected:?}, got {actual:?}"));
    }
}

fn push_true(failures: &mut Vec<String>, path: &str, actual: bool) {
    if !actual {
        failures.push(format!("{path}: expected true, got false"));
    }
}

async fn assert_cleanup(
    executor: &TestWorkerExecutor,
    owned_agent_id: &OwnedAgentId,
    path: &str,
) -> Result<(), String> {
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
    .map_err(|_| format!("{path}.cleanup: timed out with active resource state"))
}

fn assert_resource_contract(
    failures: &mut Vec<String>,
    contract: ResourceContract,
    observation: &ResourceObservation,
    path: &str,
    provider: &str,
    owner_agent_id: &str,
) {
    let principal = "oidc:matrix-resource-acceptance".to_string();
    let provider = provider.to_string();
    let owner_agent_id = owner_agent_id.to_string();
    match contract {
        ResourceContract::Secret => {
            push_eq(
                failures,
                &format!("{path}.secretFirstProvider"),
                &observation.secret_first_provider,
                &provider,
            );
            push_eq(
                failures,
                &format!("{path}.secretSecondProvider"),
                &observation.secret_second_provider,
                &provider,
            );
            push_true(
                failures,
                &format!("{path}.secretFirstRevealed"),
                observation.secret_first_revealed,
            );
            push_true(
                failures,
                &format!("{path}.secretSecondRevealed"),
                observation.secret_second_revealed,
            );
            push_eq(
                failures,
                &format!("{path}.secretPrincipal"),
                &observation.secret_principal,
                &principal,
            );
            push_eq(
                failures,
                &format!("{path}.secretOwnerAgentId"),
                &observation.secret_owner_agent_id,
                &owner_agent_id,
            );
        }
        ResourceContract::Quota => {
            push_eq(
                failures,
                &format!("{path}.quotaProvider"),
                &observation.quota_provider,
                &provider,
            );
            push_true(
                failures,
                &format!("{path}.quotaReserved"),
                observation.quota_reserved,
            );
            push_true(
                failures,
                &format!("{path}.quotaReturnedUsable"),
                observation.quota_returned_usable,
            );
            push_true(
                failures,
                &format!("{path}.quotaOriginalConsumed"),
                observation.quota_original_consumed,
            );
            push_eq(
                failures,
                &format!("{path}.quotaPrincipal"),
                &observation.quota_principal,
                &principal,
            );
            push_eq(
                failures,
                &format!("{path}.quotaOwnerAgentId"),
                &observation.quota_owner_agent_id,
                &owner_agent_id,
            );
        }
        ResourceContract::Permission => {
            push_true(
                failures,
                &format!("{path}.permissionSupported"),
                observation.permission_supported,
            );
            push_eq(
                failures,
                &format!("{path}.permissionProvider"),
                &observation.permission_provider,
                &provider,
            );
            push_true(
                failures,
                &format!("{path}.permissionSameIdentity"),
                observation.permission_same_identity,
            );
            push_true(
                failures,
                &format!("{path}.permissionOriginalConsumed"),
                observation.permission_original_consumed,
            );
            push_eq(
                failures,
                &format!("{path}.permissionPrincipal"),
                &observation.permission_principal,
                &principal,
            );
            push_eq(
                failures,
                &format!("{path}.permissionOwnerAgentId"),
                &observation.permission_owner_agent_id,
                &owner_agent_id,
            );
        }
        ResourceContract::TypedStream => push_eq(
            failures,
            &format!("{path}.typedValues"),
            &observation.typed_values,
            &vec![7, 16, 28],
        ),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_resource_contract(
    contract: ResourceContract,
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    rust_provider: &PrecompiledComponent,
    rust_caller: &PrecompiledComponent,
    typescript_provider: &PrecompiledComponent,
    typescript_caller: &PrecompiledComponent,
    scala: &PrecompiledComponent,
    moonbit: &PrecompiledComponent,
    effect_provider: &PrecompiledComponent,
    effect_caller: &PrecompiledComponent,
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
        Provider {
            language: "rust",
            component: rust_provider,
            package: "golem-it:tool-streaming-rust-provider",
            supports_secret: true,
            supports_named_quota: true,
        },
        Provider {
            language: "typescript",
            component: typescript_provider,
            package: "golem-it:tool-streaming-ts-provider",
            supports_secret: true,
            supports_named_quota: false,
        },
        Provider {
            language: "scala",
            component: scala,
            package: "scala:examples",
            supports_secret: true,
            supports_named_quota: true,
        },
        Provider {
            language: "moonbit",
            component: moonbit,
            package: "golem:moonbit-examples",
            supports_secret: true,
            supports_named_quota: true,
        },
        Provider {
            language: "effect",
            component: effect_provider,
            package: "golem-it:tool-streaming-effect-provider",
            supports_secret: true,
            supports_named_quota: true,
        },
    ];
    let callers = [
        Caller {
            language: "rust",
            component: rust_caller,
            agent_type: "RustResourceToolStreamingCaller",
            supports_named_quota: true,
        },
        Caller {
            language: "typescript",
            component: typescript_caller,
            agent_type: "TsResourceToolStreamingCaller",
            supports_named_quota: false,
        },
        Caller {
            language: "scala",
            component: scala,
            agent_type: "ScalaResourceToolStreamingCaller",
            supports_named_quota: true,
        },
        Caller {
            language: "moonbit",
            component: moonbit,
            agent_type: "MoonBitResourceToolStreamingCaller",
            supports_named_quota: true,
        },
        Caller {
            language: "effect",
            component: effect_caller,
            agent_type: "EffectToolStreamingCaller",
            supports_named_quota: true,
        },
    ];

    let mut stored_providers = Vec::new();
    let mut provider_tools = Vec::new();
    for provider in &providers {
        stored_providers.push(
            executor
                .component_dep(&context.default_environment_id, provider.component)
                .store()
                .await?,
        );
        provider_tools.push(
            extract_component_metadata(
                &deps
                    .component_directory
                    .join(format!("{}.wasm", provider.component.wasm_name)),
                false,
                true,
            )
            .await?
            .tools,
        );
    }

    let mut stored_callers = Vec::new();
    for caller in &callers {
        stored_callers.push(
            executor
                .component_dep(&context.default_environment_id, caller.component)
                .store()
                .await?,
        );
    }

    let mut failures = Vec::new();
    for (caller_index, caller) in callers.iter().enumerate() {
        for (provider_index, provider) in providers.iter().enumerate() {
            let supported = match contract {
                ResourceContract::Secret => provider.supports_secret,
                ResourceContract::Quota => {
                    caller.supports_named_quota == provider.supports_named_quota
                }
                ResourceContract::Permission => true,
                ResourceContract::TypedStream => true,
            };
            if !supported {
                continue;
            }

            let path = format!("{}->{}", caller.language, provider.language);
            let caller_component = &stored_callers[caller_index];
            let provider_component = &stored_providers[provider_index];
            let mut deployment = deployment_state(
                context.account_id,
                provider_component.id,
                provider_component.revision,
                provider.package,
                caller.agent_type,
                provider_tools[provider_index]
                    .iter()
                    .filter(|tool| root_tool_name(tool) != "matrix-permission-issuer")
                    .cloned()
                    .collect(),
            );
            if matches!(contract, ResourceContract::Permission) {
                let issuer_definition = provider_tools[0]
                    .iter()
                    .find(|tool| root_tool_name(tool) == "matrix-permission-issuer")
                    .expect("Rust provider exports matrix-permission-issuer")
                    .clone();
                let rust_provider_component = &stored_providers[0];
                let issuer_deployment = deployment_state(
                    context.account_id,
                    rust_provider_component.id,
                    rust_provider_component.revision,
                    providers[0].package,
                    caller.agent_type,
                    vec![issuer_definition],
                );
                deployment
                    .registered_tools
                    .extend(issuer_deployment.registered_tools);
                for (owner, bindings) in issuer_deployment.tool_bindings {
                    deployment
                        .tool_bindings
                        .entry(owner)
                        .or_default()
                        .extend(bindings);
                }
            }
            environment_state.set_tool_deployment(
                context.default_environment_id,
                caller_component.id,
                caller_component.revision,
                Some(deployment),
            );
            let agent_id = agent_id!(
                caller.agent_type,
                format!("resource-acceptance-{}-{provider_index}", caller.language)
            );
            let worker_id = executor
                .start_agent(&caller_component.id, agent_id.clone())
                .await?;
            let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
            if owned_agent_id.environment_id != context.default_environment_id {
                failures.push(format!(
                    "{path}.environment: invocation escaped its environment"
                ));
            }

            let method = match contract {
                ResourceContract::Secret => "matrix_secret_observation",
                ResourceContract::Quota => "matrix_quota_observation",
                ResourceContract::Permission => "matrix_permission_observation",
                ResourceContract::TypedStream => "matrix_typed_stream_observation",
            };
            match executor
                .invoke_and_await_agent_as_principal(
                    caller_component,
                    &agent_id,
                    oidc_principal(),
                    method,
                    data_value!(),
                )
                .await
            {
                Ok(value) => match value.into_typed::<ResourceObservation>() {
                    Ok(observation) => assert_resource_contract(
                        &mut failures,
                        contract,
                        &observation,
                        &path,
                        provider.language,
                        &agent_id.to_string(),
                    ),
                    Err(error) => failures.push(format!("{path}.decode: {error:#}")),
                },
                Err(error) => failures.push(format!("{path}.invoke: {error:#}")),
            }
            if let Err(error) = assert_cleanup(&executor, &owned_agent_id, &path).await {
                failures.push(error);
            }
        }
    }

    assert!(
        failures.is_empty(),
        "matrix client resource acceptance failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

macro_rules! resource_test {
    ($name:ident, $contract:expr) => {
        #[test]
        #[tracing::instrument]
        #[timeout("15m")]
        async fn $name(
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
            run_resource_contract(
                $contract,
                last_unique_id,
                deps,
                rust_provider,
                rust_caller,
                typescript_provider,
                typescript_caller,
                scala,
                moonbit,
                effect_provider,
                effect_caller,
            )
            .await
        }
    };
}

resource_test!(
    named_secret_follow_up_and_cleanup_across_supported_rotations,
    ResourceContract::Secret
);
resource_test!(
    quota_transfer_preserves_owner_and_environment,
    ResourceContract::Quota
);
resource_test!(
    permission_card_transfer_preserves_identity_owner_and_environment,
    ResourceContract::Permission
);
resource_test!(
    typed_streams_have_exact_values_and_cleanup_across_all_rotations,
    ResourceContract::TypedStream
);

#[derive(Debug, FromSchema)]
struct TypeScriptByteEvidence {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result_terminal: String,
}

#[derive(Debug, FromSchema)]
struct ScalaByteEvidence {
    stdout: Vec<i32>,
    stderr: Vec<i32>,
    result: String,
}

#[derive(Debug, FromSchema)]
struct MoonBitByteEvidence {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result: String,
}

#[test]
#[tracing::instrument]
#[timeout("10m")]
async fn byte_streams_have_exact_tuples_terminals_and_cleanup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] rust_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] rust_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_provider")] typescript_provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_caller")] typescript_caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_scala")] scala: &PrecompiledComponent,
    #[tagged_as("tool_streaming_moonbit")] moonbit: &PrecompiledComponent,
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

    let pairs = [
        (
            "rust",
            rust_provider,
            rust_caller,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            "rust_provider_terminal_rows",
        ),
        (
            "typescript",
            typescript_provider,
            typescript_caller,
            "golem-it:tool-streaming-ts-provider",
            "TsToolStreamingCaller",
            "dualOutputDeclaredError",
        ),
        (
            "scala",
            scala,
            scala,
            "scala:examples",
            "ScalaToolStreamingCaller",
            "dualOutputDeclaredError",
        ),
        (
            "moonbit",
            moonbit,
            moonbit,
            "golem:moonbit-examples",
            "MoonBitToolStreamingCaller",
            "dual_output_declared_error",
        ),
    ];

    let mut failures = Vec::new();
    for (language, provider, caller, package, agent_type, method) in pairs {
        let provider_component = executor
            .component_dep(&context.default_environment_id, provider)
            .store()
            .await?;
        let caller_component = executor
            .component_dep(&context.default_environment_id, caller)
            .store()
            .await?;
        let tools = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
            false,
            true,
        )
        .await?
        .tools;
        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            Some(deployment_state(
                context.account_id,
                provider_component.id,
                provider_component.revision,
                package,
                agent_type,
                tools,
            )),
        );

        let agent_id = agent_id!(agent_type, format!("byte-acceptance-{language}"));
        let worker_id = executor
            .start_agent(&caller_component.id, agent_id.clone())
            .await?;
        let result = executor
            .invoke_and_await_agent(&caller_component, &agent_id, method, data_value!())
            .await;
        match (language, result) {
            ("rust", Ok(value)) => match value.into_typed::<Vec<String>>() {
                Ok(rows) => {
                    if rows.len() != 8 {
                        failures.push(format!(
                            "rust.byteTuple: expected 8 cells, got {}: {rows:?}",
                            rows.len()
                        ));
                    } else {
                        push_eq(
                            &mut failures,
                            "rust.declaredTuple",
                            &rows[..2],
                            &["marker:".to_string(), "finished".to_string()],
                        );
                        if !rows[2].contains("Declared") {
                            failures.push(format!(
                                "rust.declaredResult: expected declared error, got {:?}",
                                rows[2]
                            ));
                        }
                        push_eq(
                            &mut failures,
                            "rust.failedAndDroppedTuples",
                            &rows[3..],
                            &[
                                "marker:".to_string(),
                                "stream producer failed".to_string(),
                                "false".to_string(),
                                "marker:".to_string(),
                                "false".to_string(),
                            ],
                        );
                    }
                }
                Err(error) => failures.push(format!("rust.decode: {error:#}")),
            },
            ("typescript", Ok(value)) => match value.into_typed::<TypeScriptByteEvidence>() {
                Ok(evidence) => {
                    push_eq(
                        &mut failures,
                        "typescript.stdout",
                        &evidence.stdout,
                        &vec![0, 127, 128, 255],
                    );
                    push_eq(
                        &mut failures,
                        "typescript.stderr",
                        &evidence.stderr,
                        &vec![255, 128, 1, 2],
                    );
                    push_eq(
                        &mut failures,
                        "typescript.resultTerminal",
                        &evidence.result_terminal,
                        &"declared-error".to_string(),
                    );
                }
                Err(error) => failures.push(format!("typescript.decode: {error:#}")),
            },
            ("scala", Ok(value)) => match value.into_typed::<ScalaByteEvidence>() {
                Ok(evidence) => {
                    push_eq(
                        &mut failures,
                        "scala.stdout",
                        &evidence.stdout,
                        &vec![0, 127, 255],
                    );
                    push_eq(
                        &mut failures,
                        "scala.stderr",
                        &evidence.stderr,
                        &vec![128, 1, 2],
                    );
                    push_eq(
                        &mut failures,
                        "scala.resultTerminal",
                        &evidence.result,
                        &"declared:dual-expected".to_string(),
                    );
                }
                Err(error) => failures.push(format!("scala.decode: {error:#}")),
            },
            ("moonbit", Ok(value)) => match value.into_typed::<MoonBitByteEvidence>() {
                Ok(evidence) => {
                    push_eq(
                        &mut failures,
                        "moonbit.stdout",
                        &evidence.stdout,
                        &b"moon-out:\x00\xff".to_vec(),
                    );
                    push_eq(
                        &mut failures,
                        "moonbit.stderr",
                        &evidence.stderr,
                        &b"moon-err:\x80!".to_vec(),
                    );
                    push_eq(
                        &mut failures,
                        "moonbit.resultTerminal",
                        &evidence.result,
                        &"declared:dual-expected".to_string(),
                    );
                }
                Err(error) => failures.push(format!("moonbit.decode: {error:#}")),
            },
            (_, Err(error)) => failures.push(format!("{language}.invoke: {error:#}")),
            (other, Ok(_)) => failures.push(format!("unexpected byte language {other}")),
        }
        if let Err(error) = assert_cleanup(
            &executor,
            &OwnedAgentId::new(context.default_environment_id, &worker_id),
            language,
        )
        .await
        {
            failures.push(error);
        }
    }

    assert!(
        failures.is_empty(),
        "matrix byte-stream acceptance failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}
