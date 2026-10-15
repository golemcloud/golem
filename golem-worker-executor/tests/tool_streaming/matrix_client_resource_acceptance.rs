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

#[test]
#[timeout("120s")]
async fn gol838_entity_install_definitions_survive_next_primary_pin_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::card::{Card, CardId, CardManagedByRuntimeDerived, StoredCard};
    use golem_common::model::entity::EntityInvocationRequest;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{HostRequest, OplogEntry};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_worker_executor::services::card::{CardService, CardState};
    use golem_worker_executor::services::golem_config::SnapshotPolicy;
    use golem_worker_executor::services::oplog::OplogOps;
    use golem_worker_executor_test_utils::TestCardService;

    struct CountingCards {
        calls: AtomicUsize,
        entered: tokio::sync::mpsc::Sender<(CardId, CardManagedByRuntimeDerived)>,
        release: [tokio::sync::Semaphore; 2],
    }

    #[async_trait::async_trait]
    impl CardService for CountingCards {
        async fn record_revoked_cards(&self, ids: &[CardId]) {
            TestCardService.record_revoked_cards(ids).await;
        }

        async fn check_cards(
            &self,
            ids: Vec<CardId>,
        ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
            TestCardService.check_cards(ids).await
        }

        async fn create_runtime_card(
            &self,
            card: StoredCard,
            provenance: CardManagedByRuntimeDerived,
        ) -> Result<StoredCard, WorkerExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.release.len() {
                self.entered
                    .send((card.card_id(), provenance.clone()))
                    .await
                    .unwrap();
                self.release[call].acquire().await.unwrap().forget();
            }
            TestCardService.create_runtime_card(card, provenance).await
        }
    }

    async fn issuers(
        executor: &TestWorkerExecutor,
        owner: &OwnedAgentId,
        entries: &[OplogEntry],
    ) -> anyhow::Result<Vec<(OplogIndex, EntityInvocationRequest)>> {
        let active = executor.active_agent(owner).await.unwrap();
        let mut issuers = Vec::new();
        for (offset, entry) in entries.iter().enumerate() {
            if let OplogEntry::Start {
                function_name,
                request: Some(payload),
                ..
            } = entry
                && function_name.to_string() == "golem::entity::invoke"
            {
                let HostRequest::EntityInvocation(request) = active
                    .execution()
                    .oplog()
                    .download_payload(payload.clone())
                    .await
                    .map_err(anyhow::Error::msg)?
                else {
                    panic!("expected entity request")
                };
                let metadata =
                    desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)?;
                if metadata.entity.name() == "matrix-permission-issuer" {
                    issuers.push((OplogIndex::from_u64(offset as u64 + 1), metadata));
                }
            }
        }
        Ok(issuers)
    }

    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    environment.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["secret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String("matrix-secret-value".to_string())),
    });
    let (entered, mut derivations) = tokio::sync::mpsc::channel(2);
    let cards = Arc::new(CountingCards {
        calls: AtomicUsize::new(0),
        entered,
        release: std::array::from_fn(|_| tokio::sync::Semaphore::new(0)),
    });
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::Disabled
        })),
        environment_state_service: Some(environment.clone()),
        create_card_service: Some(Arc::new({
            let cards = cards.clone();
            move || cards.clone()
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
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "RustResourceToolStreamingCaller",
            metadata.tools,
        )),
    );
    let agent = agent_id!(
        "RustResourceToolStreamingCaller",
        "entity-install-before-primary-pin"
    );
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker);
    let id = CardId(uuid::Uuid::from_u128(1));
    let card = StoredCard::Concrete(Card {
        card_id: id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    });
    TestCardService
        .create_runtime_card(
            card.clone(),
            CardManagedByRuntimeDerived {
                environment_id: context.default_environment_id,
                agent_id: worker.clone(),
                invocation_key: IdempotencyKey::fresh(),
                oplog_index: OplogIndex::INITIAL,
            },
        )
        .await?;
    let first = IdempotencyKey::fresh();
    let second = IdempotencyKey::fresh();
    let mut body = executor.gate_next_entity_body_start(&worker);
    let invocation = executor.invoke_and_await_agent_with_key(
        &caller_component,
        &agent,
        &first,
        "matrix_parallel_permission_issuance",
        data_value!(),
    );
    let install = async {
        tokio::time::timeout(Duration::from_secs(20), body.entered()).await?;
        let (first_card, first_derive) =
            tokio::time::timeout(Duration::from_secs(20), derivations.recv())
                .await?
                .unwrap();
        executor.commit_oplog(&worker).await?;
        let entries = executor.stored_oplog(&worker).await;
        let derive_starts = |entries: &[OplogEntry]| {
            entries.iter().enumerate().filter_map(|(offset, entry)| {
                matches!(entry, OplogEntry::Start { function_name, parent_start_index: Some(_), .. }
                    if function_name.to_string() == "golem::permissions::derive::persist-derived-card")
                    .then_some(OplogIndex::from_u64(offset as u64 + 1))
            }).collect::<Vec<_>>()
        };
        let first_starts = derive_starts(&entries);
        assert_eq!(first_starts.len(), 1);
        let first_start = first_starts[0];
        assert_eq!(first_derive.agent_id, worker);
        let issuer = issuers(&executor, &owner, &entries).await?;
        assert_eq!(issuer.len(), 2);
        assert!(
            issuer[0]
                .1
                .authority_wallet
                .iter()
                .any(|parent| parent.card_id() > id)
        );
        assert!(!issuer[0].1.authority_wallet.contains(&card));
        assert!(!issuer[1].1.authority_wallet.contains(&card));
        executor.queue_card_install(&worker, card.clone()).await?;
        body.release();
        let (second_card, second_derive) =
            tokio::time::timeout(Duration::from_secs(20), derivations.recv())
                .await?
                .unwrap();
        assert_ne!(first_card, second_card);
        assert_eq!(second_derive.agent_id, worker);
        executor.commit_oplog(&worker).await?;
        let starts = derive_starts(&executor.stored_oplog(&worker).await);
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0], first_start);
        let second_start = starts[1];
        cards.release[1].add_permits(1);
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let entries = executor.stored_oplog(&worker).await;
                if entries.iter().any(|entry| {
                    matches!(entry, OplogEntry::CardDerived { card, wallet_generation: 1, .. } if card.card_id() == second_card)
                }) {
                    assert!(!entries.iter().any(|entry| {
                        matches!(entry, OplogEntry::End { start_index, .. } if *start_index == first_start)
                    }));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        cards.release[0].add_permits(1);
        anyhow::Ok((first_start, second_start, first_card, second_card))
    };
    let (result, installed) = tokio::join!(invocation, install);
    let (derived, second_derived, first_card, second_card) = installed?;
    assert!(result?.into_typed::<bool>()?);
    assert!(
        executor
            .invoke_and_await_agent_with_key(
                &caller_component,
                &agent,
                &second,
                "matrix_permission_observation",
                data_value!()
            )
            .await?
            .into_typed::<ResourceObservation>()?
            .permission_same_identity
    );
    executor.commit_oplog(&worker).await?;
    let recorded = executor.stored_oplog(&worker).await;
    let issuer = issuers(&executor, &owner, &recorded).await?;
    assert_eq!(issuer.len(), 3);
    assert!(!issuer[0].1.authority_wallet.contains(&card));
    assert!(!issuer[1].1.authority_wallet.contains(&card));
    assert!(issuer[2].1.authority_wallet.contains(&card));
    let primary = |key: &IdempotencyKey| {
        recorded
            .iter()
            .enumerate()
            .find_map(|(offset, entry)| match entry {
                OplogEntry::AgentInvocationStarted {
                    idempotency_key,
                    wallet_pin,
                    ..
                } if idempotency_key == key => Some((
                    OplogIndex::from_u64(offset as u64 + 1),
                    wallet_pin.as_ref().clone(),
                )),
                _ => None,
            })
            .unwrap()
    };
    let p1 = primary(&first);
    let p2 = primary(&second);
    assert!(!p1.1.pinned_card_ids.contains(&id));
    assert!(p2.1.pinned_card_ids.contains(&id));
    assert_eq!(p2.1.wallet_token.generation, 1);
    let index_of = |condition: &dyn Fn(&OplogEntry) -> bool| {
        OplogIndex::from_u64(recorded.iter().position(condition).unwrap() as u64 + 1)
    };
    let queued = index_of(
        &|entry| matches!(entry, OplogEntry::CardEventQueued { event, .. } if event.card_id() == id),
    );
    let installed = index_of(
        &|entry| matches!(entry, OplogEntry::CardInstalled { card: value, wallet_generation: 1, .. } if value.as_ref() == &card),
    );
    let parent_of = |start: OplogIndex| match &recorded[(u64::from(start) - 1) as usize] {
        OplogEntry::Start {
            parent_start_index: Some(parent),
            function_name,
            ..
        } if function_name.to_string() == "golem::permissions::derive::persist-derived-card" => {
            *parent
        }
        entry => panic!("expected genuine entity derive Start at {start}, got {entry:?}"),
    };
    let first_parent = parent_of(derived);
    let second_parent = parent_of(second_derived);
    assert_ne!(first_parent, second_parent);
    let clock = index_of(
        &|entry| matches!(entry, OplogEntry::Start { function_name: HostFunctionName::WallClockNow, parent_start_index: Some(parent), .. } if *parent == second_parent),
    );
    let first_end = index_of(
        &|entry| matches!(entry, OplogEntry::End { start_index, .. } if *start_index == derived),
    );
    let second_end = index_of(
        &|entry| matches!(entry, OplogEntry::End { start_index, .. } if *start_index == second_derived),
    );
    let first_audit = index_of(
        &|entry| matches!(entry, OplogEntry::CardDerived { card, entity_parent_start_index: Some(parent), wallet_generation: 1, .. } if card.card_id() == first_card && *parent == first_parent),
    );
    let second_audit = index_of(
        &|entry| matches!(entry, OplogEntry::CardDerived { card, entity_parent_start_index: Some(parent), wallet_generation: 1, .. } if card.card_id() == second_card && *parent == second_parent),
    );
    assert_eq!(second_audit, second_end.next());
    assert!(
        p1.0 < first_parent
            && first_parent < derived
            && derived < queued
            && queued < installed
            && installed < clock
            && clock < second_derived
            && second_derived < second_end
            && second_end < second_audit
            && second_audit < first_end
            && first_end < first_audit
            && first_audit < p2.0
    );
    assert!(
        !recorded
            .iter()
            .any(|entry| matches!(entry, OplogEntry::Snapshot { .. }))
    );
    let writes = cards.calls.load(Ordering::SeqCst);
    assert_eq!(writes, 3);
    let expected_second_card = match &recorded[(u64::from(second_audit) - 1) as usize] {
        OplogEntry::CardDerived { card, .. } => card.as_ref().clone(),
        _ => unreachable!(),
    };
    executor.shutdown_and_wait_for_invocation_loops().await?;

    let executor = start_with_overrides(deps, &context, overrides).await?;
    let first_claim = executor.gate_entity_reconstruction_claim_at(&worker, first_parent);
    let second_body = executor.gate_next_entity_body_start(&worker);
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent,
        "matrix_permission_observation",
        data_value!(),
    );
    let replay = async {
        let mut first_claim = first_claim;
        let mut second_body = second_body;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(20), first_claim.entered()).await?,
            first_parent
        );
        tokio::time::timeout(Duration::from_secs(20), second_body.entered()).await?;
        let active = executor.active_agent(&owner).await.unwrap();
        let execution = active.execution();
        let (horizon, _, _) = execution.test_pending_card_derivation(second_card).await?;
        let (first_lookup, release_first) =
            executor.gate_next_direct_card_audit_read(&worker, derived, horizon);
        let (second_lookup, release_second) =
            executor.gate_next_direct_card_audit_read(&worker, second_derived, horizon);
        second_body.release();
        tokio::time::timeout(Duration::from_secs(20), second_lookup).await??;
        let (observed_horizon, cursor, pending) =
            execution.test_pending_card_derivation(second_card).await?;
        assert_eq!(observed_horizon, horizon);
        assert!(cursor >= second_audit);
        assert_eq!(pending, Some((expected_second_card, 1)));
        first_claim.release();
        tokio::time::timeout(Duration::from_secs(20), first_lookup).await??;
        let (observed_horizon, cursor, pending) =
            execution.test_pending_card_derivation(second_card).await?;
        assert_eq!(observed_horizon, horizon);
        assert!(cursor >= first_audit);
        assert_eq!(pending, None);
        assert_eq!(cards.calls.load(Ordering::SeqCst), writes);
        release_second.send(()).unwrap();
        release_first.send(()).unwrap();
        anyhow::Ok(())
    };
    let (result, ()) = tokio::try_join!(invocation, replay)?;
    assert!(
        result
            .into_typed::<ResourceObservation>()?
            .permission_same_identity
    );
    executor.commit_oplog(&worker).await?;
    let recovered = executor.stored_oplog(&worker).await;
    assert_eq!(&recovered[..recorded.len()], &recorded);
    let last_pin = recovered
        .iter()
        .rev()
        .find_map(|entry| match entry {
            OplogEntry::AgentInvocationStarted { wallet_pin, .. } => Some(wallet_pin),
            _ => None,
        })
        .unwrap();
    assert_eq!(last_pin.wallet_token.generation, 1);
    assert!(last_pin.pinned_card_ids.contains(&id));
    // Only the new live invocation derives another card; completed replay must not write one.
    assert_eq!(cards.calls.load(Ordering::SeqCst), writes + 1);
    let active = executor.active_agent(&owner).await.unwrap();
    assert!(active.primary().get_wallet_cards().await?.contains(&card));
    Ok(())
}

#[test]
#[timeout("180s")]
async fn gol838_entity_mutations_reconstruct_from_oplog_and_exact_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::AgentEvent;
    use golem_common::model::card::{
        Card, CardId, CardManagedByRuntimeDerived, PolymorphicPermissionPattern, StoredCard,
        parse_permission_fields, parse_polymorphic_permission,
    };
    use golem_common::model::entity::EntityInvocationRequest;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{HostRequest, OplogEntry};
    use golem_worker_executor::services::card::CardService;
    use golem_worker_executor::services::golem_config::SnapshotPolicy;
    use golem_worker_executor::services::oplog::OplogOps;

    for snapshot in [false, true] {
        for revoke in [false, true] {
            let context = TestContext::new(last_unique_id);
            let environment = Arc::new(TestEnvironmentStateService::default());
            let cards = Arc::new(crate::hot_update::SaveHookCardService::default());
            let deliveries = Arc::new(std::sync::Mutex::new(Vec::new()));
            let overrides = TestExecutorOverrides {
                configure: Some(Arc::new(move |config| {
                    config.oplog.default_snapshotting = if snapshot {
                        SnapshotPolicy::EveryNInvocation { count: 1 }
                    } else {
                        SnapshotPolicy::Disabled
                    };
                })),
                environment_state_service: Some(environment.clone()),
                create_card_service: Some(Arc::new({
                    let cards = cards.clone();
                    move || cards.clone()
                })),
                // Count source sends; the target does not execute in this test.
                wrap_worker_proxy: Some(Arc::new({
                    let deliveries = deliveries.clone();
                    move |inner| {
                        crate::api::recording_accepting_card_transfer_proxy(
                            inner,
                            deliveries.clone(),
                        )
                    }
                })),
                ..Default::default()
            };
            let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
            let provider_component = executor
                .component_dep(&context.default_environment_id, provider)
                .store()
                .await?;
            let component = executor
                .component_dep(&context.default_environment_id, caller)
                .update_agent_provision_config("WalletRecoveryProbe", |config| {
                    let grants = &mut config.initial_permissions.lower_bound.positive;
                    grants
                        .retain(|grant| !matches!(grant, PolymorphicPermissionPattern::Config(_)));
                    grants.push(
                        parse_polymorphic_permission("config(?agent) @ * : read : control")
                            .unwrap(),
                    );
                })
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
            environment.set_tool_deployment(
                context.default_environment_id,
                component.id,
                component.revision,
                Some(deployment_state(
                    context.account_id,
                    provider_component.id,
                    provider_component.revision,
                    "golem-it:tool-streaming-rust-provider",
                    "WalletRecoveryProbe",
                    metadata.tools,
                )),
            );
            let agent = agent_id!(
                "WalletRecoveryProbe",
                format!("mutation-{snapshot}-{revoke}")
            );
            let worker = executor.start_agent(&component.id, agent.clone()).await?;
            let owner = OwnedAgentId::new(context.default_environment_id, &worker);
            let id = CardId::new();
            let schema_id = golem_schema::model::CardId { uuid: id.0 };
            let card = StoredCard::Concrete(Card {
                card_id: id,
                parent_ids: Vec::new(),
                lower_positive: vec![
                    parse_permission_fields("config", "*/*/*/*/*", "*", "read", "probe").unwrap(),
                ],
                lower_negative: Vec::new(),
                upper_positive: Vec::new(),
                upper_negative: Vec::new(),
                created_at: chrono::Utc::now(),
                expires_at: None,
                system_card: false,
                managed_by: None,
            });
            cards
                .create_runtime_card(
                    card.clone(),
                    CardManagedByRuntimeDerived {
                        environment_id: context.default_environment_id,
                        agent_id: worker.clone(),
                        invocation_key: IdempotencyKey::fresh(),
                        oplog_index: OplogIndex::INITIAL,
                    },
                )
                .await?;
            let target = if revoke {
                String::new()
            } else {
                agent_id!("WalletRecoveryProbe", "target").to_string()
            };
            let mut body = executor.gate_next_entity_body_start(&worker);
            let invocation = executor.invoke_and_await_agent(
                &component,
                &agent,
                "change",
                data_value!(schema_id, component.id, target),
            );
            let install = async {
                tokio::time::timeout(Duration::from_secs(20), body.entered()).await?;
                executor.queue_card_install(&worker, card.clone()).await?;
                body.release();
                anyhow::Ok(())
            };
            let (result, ()) = tokio::try_join!(invocation, install)?;
            assert_eq!(
                result.into_typed::<(bool, u64, bool, bool)>()?,
                (false, 2, true, true)
            );
            executor.commit_oplog(&worker).await?;
            let entries = executor.stored_oplog(&worker).await;
            let (entity_start, payload) = entries
                .iter()
                .enumerate()
                .find_map(|(offset, entry)| match entry {
                    OplogEntry::Start {
                        function_name,
                        request: Some(payload),
                        ..
                    } if function_name.to_string() == "golem::entity::invoke" => {
                        Some((OplogIndex::from_u64(offset as u64 + 1), payload.clone()))
                    }
                    _ => None,
                })
                .unwrap();
            let active = executor.active_agent(&owner).await.unwrap();
            let HostRequest::EntityInvocation(request) = active
                .execution()
                .oplog()
                .download_payload(payload)
                .await
                .map_err(anyhow::Error::msg)?
            else {
                panic!("expected real entity request");
            };
            let request = desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)?;
            assert_eq!(request.entity.name(), "wallet-mutation");
            assert!(
                !request
                    .authority_wallet
                    .iter()
                    .any(|card| card.card_id() == id)
            );
            drop(active);
            let index_of = |predicate: &dyn Fn(&OplogEntry) -> bool| {
                OplogIndex::from_u64(entries.iter().position(predicate).unwrap() as u64 + 1)
            };
            let installed = index_of(&|entry| {
                matches!(entry,
                OplogEntry::CardInstalled { card: value, wallet_generation: 1, .. }
                    if value.as_ref() == &card)
            });
            let clock = index_of(&|entry| {
                matches!(entry,
                OplogEntry::Start { function_name: HostFunctionName::WallClockNow,
                    parent_start_index: Some(parent), .. } if *parent == entity_start)
            });
            assert!(entity_start < installed && installed < clock);
            let mutation_name = if revoke {
                HostFunctionName::GolemPermissionsRevokePersist
            } else {
                HostFunctionName::GolemPermissionsInstallTransfer
            };
            assert!(entries.iter().any(|entry| matches!(entry,
                OplogEntry::Start { function_name, parent_start_index: Some(parent), .. }
                    if *function_name == mutation_name && *parent == entity_start)));
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| match entry {
                        OplogEntry::CardRevokedCascade {
                            revoked_card_ids,
                            local_wallet_generation: 2,
                            entity_parent_start_index,
                            ..
                        } if revoke =>
                            revoked_card_ids == &[id]
                                && *entity_parent_start_index == Some(entity_start),
                        OplogEntry::CardTransferStarted {
                            card_id,
                            source_wallet_generation: 2,
                            entity_parent_start_index,
                            ..
                        } if !revoke =>
                            *card_id == id && *entity_parent_start_index == Some(entity_start),
                        _ => false,
                    })
                    .count(),
                1
            );
            let selected = if snapshot {
                let selected =
                    crate::hot_update::wait_for_snapshot_after(&executor, &worker, entity_start)
                        .await?;
                let records = executor.stored_oplog(&worker).await;
                let OplogEntry::Snapshot {
                    active_cards,
                    wallet_generation,
                    ..
                } = &records[selected.as_u64() as usize - 1]
                else {
                    panic!("expected selected snapshot");
                };
                assert_eq!(*wallet_generation, 2);
                assert!(!active_cards.iter().any(|card| card.card_id() == id));
                Some(selected)
            } else {
                assert_eq!(
                    executor
                        .invoke_and_await_agent(
                            &component,
                            &agent,
                            "observe",
                            data_value!(schema_id),
                        )
                        .await?
                        .into_typed::<(bool, u64, bool, bool)>()?,
                    (false, 2, true, true)
                );
                None
            };
            executor.commit_oplog(&worker).await?;
            let recorded = executor.stored_oplog(&worker).await;
            if !snapshot {
                assert!(
                    !recorded
                        .iter()
                        .any(|entry| matches!(entry, OplogEntry::Snapshot { .. }))
                );
                let pin = recorded
                    .iter()
                    .rev()
                    .find_map(|entry| match entry {
                        OplogEntry::AgentInvocationStarted { wallet_pin, .. } => Some(wallet_pin),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(pin.wallet_token.generation, 2);
                assert!(!pin.pinned_card_ids.contains(&id));
            }
            let sends = deliveries.lock().unwrap().len();
            let revokes = cards.revoke_calls.load(Ordering::SeqCst);
            assert_eq!(sends, usize::from(!revoke));
            assert_eq!(revokes, usize::from(revoke));
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            let executor = start_with_overrides(deps, &context, overrides).await?;
            let mut events = executor.capture_output(&worker).await?;
            assert_eq!(
                executor
                    .invoke_and_await_agent(&component, &agent, "observe", data_value!(schema_id),)
                    .await?
                    .into_typed::<(bool, u64, bool, bool)>()?,
                (false, 2, true, true)
            );
            if let Some(selected) = selected {
                let loaded = tokio::time::timeout(Duration::from_secs(10), async {
                    while let Some(event) = events.recv().await {
                        match AgentEvent::try_from(event) {
                            Ok(AgentEvent::SnapshotRecoverySucceeded {
                                snapshot_index, ..
                            }) => return snapshot_index,
                            Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                                panic!("snapshot recovery failed: {error}")
                            }
                            _ => {}
                        }
                    }
                    panic!("missing exact snapshot load");
                })
                .await?;
                assert_eq!(loaded, selected);
            }
            executor.commit_oplog(&worker).await?;
            let recovered = executor.stored_oplog(&worker).await;
            assert_eq!(&recovered[..recorded.len()], &recorded);
            assert_eq!(deliveries.lock().unwrap().len(), sends);
            assert_eq!(cards.revoke_calls.load(Ordering::SeqCst), revokes);
        }
    }
    Ok(())
}
