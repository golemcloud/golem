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
use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use golem_common::agent_id;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::agent::{AgentTypeName, GolemUserPrincipal, Principal};
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::quota::{Reservation, ReserveResult, ResourceName};
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolBindingOwner, ToolDeploymentState,
    ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::model::tool_middleware::{
    CompiledToolMiddlewareChain, CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware,
    ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::model::{AgentInvocationResult, IdempotencyKey, OwnedAgentId};
use golem_common::schema::tool::{ToolMiddleware, ToolMiddlewareScope};
use golem_common::schema::{FromSchema, SchemaValue, TypedSchemaValue};
use golem_service_base::model::agent_secret::AgentSecret;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::quota::{LeaseInterest, QuotaService, UnlimitedQuotaService};
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_runtime_bypass_owner")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_runtime_bypass_provider")]
    PrecompiledComponent
);

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct NestedEvidence {
    label: String,
    ordinal: u64,
}

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct ChainEvidence {
    claimed_principal: String,
    actual_principal: String,
    owner_config: String,
    owner_secret: String,
    nested: NestedEvidence,
}

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct ValidationEvidence {
    label: String,
    ordinal: u64,
    stdout: Vec<u8>,
}

#[derive(Default)]
struct RecordingQuotaService {
    acquisitions: Mutex<Vec<(EnvironmentId, ResourceName, u64)>>,
}

impl RecordingQuotaService {
    fn acquisitions(&self) -> Vec<(EnvironmentId, ResourceName, u64)> {
        self.acquisitions.lock().unwrap().clone()
    }
}

#[async_trait]
impl QuotaService for RecordingQuotaService {
    async fn acquire(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        expected_use: u64,
        previous_credit: i64,
        previous_credit_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> LeaseInterest {
        self.acquisitions.lock().unwrap().push((
            environment_id,
            resource_name.clone(),
            expected_use,
        ));
        UnlimitedQuotaService
            .acquire(
                environment_id,
                resource_name,
                expected_use,
                previous_credit,
                previous_credit_at,
            )
            .await
    }

    async fn try_reserve(&self, interest: &mut LeaseInterest, amount: u64) -> ReserveResult {
        UnlimitedQuotaService.try_reserve(interest, amount).await
    }

    async fn commit(&self, interest: &mut LeaseInterest, reservation: Reservation, used: u64) {
        UnlimitedQuotaService
            .commit(interest, reservation, used)
            .await
    }
}

struct Fixture {
    executor: TestWorkerExecutor,
    owner_component: golem_common::model::component::ComponentDto,
    owner_id: golem_common::model::AgentId,
    other_owner_id: golem_common::model::AgentId,
    cross_environment_owner_id: golem_common::model::AgentId,
    account_id: AccountId,
    environment_id: EnvironmentId,
    other_environment_id: EnvironmentId,
    fingerprint: golem_common::model::AgentFingerprint,
    other_fingerprint: golem_common::model::AgentFingerprint,
    cross_environment_fingerprint: golem_common::model::AgentFingerprint,
    definitions: BTreeMap<ToolName, golem_common::schema::tool::Tool>,
    effects: Arc<Mutex<Vec<String>>>,
    quota: Arc<RecordingQuotaService>,
    _effect_server: tokio_util::task::AbortOnDropHandle<()>,
}

async fn fixture(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    owner: &PrecompiledComponent,
    provider: &PrecompiledComponent,
) -> anyhow::Result<Fixture> {
    let context = TestContext::new(last_unique_id);
    let publisher_environment_id = EnvironmentId::new();
    let other_environment_id = EnvironmentId::new();
    let environment = Arc::new(TestEnvironmentStateService::default());
    let quota = Arc::new(RecordingQuotaService::default());
    let (effect_port, effect_server, effects) = start_effect_server().await?;
    for (environment_id, value) in [
        (publisher_environment_id, "publisher-secret"),
        (context.default_environment_id, "caller-one-secret"),
        (other_environment_id, "caller-two-secret"),
    ] {
        environment.set_agent_secret(AgentSecret {
            id: AgentSecretId::new(),
            environment_id,
            path: CanonicalAgentSecretPath(vec!["contextSecret".to_string()]),
            revision: AgentSecretRevision::INITIAL,
            secret_type: golem_common::schema::SchemaGraph::anonymous(
                golem_common::schema::SchemaType::string(),
            ),
            secret_value: Some(SchemaValue::String(value.to_string())),
        });
    }
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            quota_service: Some(quota.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&publisher_environment_id, provider)
        .store()
        .await?;
    let owner_component = executor
        .component_dep(&context.default_environment_id, owner)
        .store()
        .await?;
    let other_owner_component = executor
        .component_dep(&other_environment_id, owner)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &publisher_environment_id,
            "golem_it_tool_runtime_bypass_middleware_release",
        )
        .name("golem-it:tool-runtime-bypass-middleware")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_runtime_bypass_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let definitions = provider_metadata
        .tools
        .iter()
        .map(|definition| {
            let name = definition.commands.nodes[0].name.clone();
            (ToolName::try_from(name).unwrap(), definition.clone())
        })
        .collect::<BTreeMap<_, _>>();
    let agent_type = AgentTypeName("ToolRuntimeBypassOwner".to_string());
    for (environment_id, component) in [
        (context.default_environment_id, &owner_component),
        (other_environment_id, &other_owner_component),
    ] {
        let mut deployment = deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            provider_metadata.tools.clone(),
        );
        install_chain(
            &mut deployment,
            &agent_type,
            &ToolName::try_from("chain-probe").unwrap(),
            middleware_component.id,
            middleware_component.revision,
            &middleware_metadata.tool_middlewares,
            &[
                "runtime-bypass-u1",
                "runtime-bypass-u2",
                "runtime-bypass-p1",
                "runtime-bypass-p2",
            ],
        );
        install_chain(
            &mut deployment,
            &agent_type,
            &ToolName::try_from("validation-probe").unwrap(),
            middleware_component.id,
            middleware_component.revision,
            &middleware_metadata.tool_middlewares,
            &["runtime-bypass-u2"],
        );
        environment.set_tool_deployment(
            environment_id,
            component.id,
            component.revision,
            Some(deployment),
        );
    }
    let agent_id = agent_id!("ToolRuntimeBypassOwner", "e2-owner");
    let owner_id = executor
        .start_agent_with(
            &owner_component.id,
            agent_id,
            HashMap::from([
                ("OWNER_MARKER".to_string(), "caller-one-config".to_string()),
                ("EFFECT_PORT".to_string(), effect_port.to_string()),
            ]),
            Vec::new(),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&owner_id).await?.fingerprint;
    let other_owner_id = executor
        .start_agent_with(
            &owner_component.id,
            agent_id!("ToolRuntimeBypassOwner", "e2-other"),
            HashMap::from([
                ("OWNER_MARKER".to_string(), "caller-one-config".to_string()),
                ("EFFECT_PORT".to_string(), effect_port.to_string()),
            ]),
            Vec::new(),
        )
        .await?;
    let other_fingerprint = executor
        .get_worker_metadata(&other_owner_id)
        .await?
        .fingerprint;
    let cross_environment_owner_id = executor
        .start_agent_with(
            &other_owner_component.id,
            agent_id!("ToolRuntimeBypassOwner", "e2-cross-environment"),
            HashMap::from([
                ("OWNER_MARKER".to_string(), "caller-two-config".to_string()),
                ("EFFECT_PORT".to_string(), effect_port.to_string()),
            ]),
            Vec::new(),
        )
        .await?;
    let cross_environment_fingerprint = executor
        .get_worker_metadata(&cross_environment_owner_id)
        .await?
        .fingerprint;
    Ok(Fixture {
        executor,
        owner_component,
        owner_id,
        other_owner_id,
        cross_environment_owner_id,
        account_id: context.account_id,
        environment_id: context.default_environment_id,
        other_environment_id,
        fingerprint,
        other_fingerprint,
        cross_environment_fingerprint,
        definitions,
        effects,
        quota,
        _effect_server: effect_server,
    })
}

#[test]
#[timeout("3m")]
async fn immediate_successor_overlap_and_calling_owner_context(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = fixture(last_unique_id, deps, owner, provider).await?;
    let normal = invoke_chain(&fixture, "forged-principal", "normal").await?;
    assert_eq!(normal.claimed_principal, "P2>P1>U2>U1>forged-principal");
    assert_eq!(normal.owner_config, "caller-one-config");
    assert_eq!(normal.owner_secret, "caller-one-secret");
    let account_bits = fixture.account_id.0.as_u128();
    assert!(normal.actual_principal.contains("Principal::GolemUser"));
    assert!(
        normal
            .actual_principal
            .contains(&format!("high-bits: {}", account_bits >> 64)),
        "unexpected middleware principal: {}",
        normal.actual_principal
    );
    assert!(
        normal
            .actual_principal
            .contains(&format!("low-bits: {}", account_bits & u64::MAX as u128)),
        "unexpected middleware principal: {}",
        normal.actual_principal
    );
    assert!(!normal.actual_principal.contains("forged-principal"));
    assert_eq!(normal.nested.label, "U1>U2>P1>P2>normal");

    let overlap = invoke_chain(&fixture, "spoofed", "overlap").await?;
    assert_eq!(
        overlap.nested.label,
        "U1[U2>P1>P2>overlap|U2>P1>P2>overlap]"
    );
    assert_eq!(fixture.effects.lock().unwrap().len(), 3);
    let acquisitions = fixture.quota.acquisitions();
    assert_eq!(acquisitions.len(), 3);
    assert!(
        acquisitions
            .iter()
            .all(|(environment, resource, expected)| {
                *environment == fixture.environment_id
                    && resource.0 == "context-quota"
                    && *expected == 7
            })
    );

    let oplog = fixture
        .executor
        .get_oplog(&fixture.owner_id, OplogIndex::INITIAL)
        .await?;
    let entity_names = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke" => {
                match &entry.attribution {
                    golem_common::model::oplog::PublicOplogEntryAttribution::Entity(entity) => {
                        Some(entity.invocation.entity.name.clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(entity_names.windows(5).any(|names| names
        == [
            "runtime-bypass-u1",
            "runtime-bypass-u2",
            "runtime-bypass-p1",
            "runtime-bypass-p2",
            "chain-probe",
        ]));
    Ok(())
}

#[test]
#[timeout("3m")]
async fn calling_owner_context_isolated_across_environments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = fixture(last_unique_id, deps, owner, provider).await?;

    let first = invoke_chain(&fixture, "first-forged-principal", "first").await?;
    let second = invoke_chain_for(
        &fixture,
        &fixture.cross_environment_owner_id,
        fixture.cross_environment_fingerprint,
        "second-forged-principal",
        "second",
    )
    .await?;
    let first_again = invoke_chain(&fixture, "first-again-forged-principal", "first-again").await?;

    for (evidence, config, secret, forged) in [
        (
            &first,
            "caller-one-config",
            "caller-one-secret",
            "first-forged-principal",
        ),
        (
            &second,
            "caller-two-config",
            "caller-two-secret",
            "second-forged-principal",
        ),
        (
            &first_again,
            "caller-one-config",
            "caller-one-secret",
            "first-again-forged-principal",
        ),
    ] {
        assert_eq!(evidence.owner_config, config);
        assert_eq!(evidence.owner_secret, secret);
        assert!(evidence.actual_principal.contains("Principal::GolemUser"));
        assert!(!evidence.actual_principal.contains(forged));
    }

    let acquisitions = fixture.quota.acquisitions();
    assert_eq!(acquisitions.len(), 3);
    assert_eq!(acquisitions[0].0, fixture.environment_id);
    assert_eq!(acquisitions[1].0, fixture.other_environment_id);
    assert_eq!(acquisitions[2].0, fixture.environment_id);
    assert!(
        acquisitions
            .iter()
            .all(|(_, resource, expected)| resource.0 == "context-quota" && *expected == 7)
    );
    assert_eq!(leaf_effect_count(&fixture), 3);
    Ok(())
}

#[test]
#[timeout("3m")]
async fn underlying_handle_is_revoked_after_return_and_cannot_cross_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = Arc::new(fixture(last_unique_id, deps, owner, provider).await?);
    let stale_fixture = fixture.clone();
    let stale = tokio::spawn(async move { invoke_chain(&stale_fixture, "stale", "stale").await });
    let checkpoint = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(index) = fixture
                .effects
                .lock()
                .unwrap()
                .iter()
                .find_map(|effect| effect.strip_prefix("stale-ready/")?.parse::<u64>().ok())
            {
                break index;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stale handle checkpoint");

    let fresh = invoke_chain_for(
        &fixture,
        &fixture.other_owner_id,
        fixture.other_fingerprint,
        "fresh",
        "normal",
    )
    .await?;
    assert_eq!(fresh.nested.label, "U1>U2>P1>P2>normal");
    assert_eq!(leaf_effect_count(&fixture), 1);

    fixture
        .executor
        .complete_promise(
            &golem_common::model::PromiseId {
                agent_id: fixture.owner_id.clone(),
                oplog_idx: OplogIndex::from_u64(checkpoint),
            },
            Vec::new(),
        )
        .await?;
    let stale_error = tokio::time::timeout(std::time::Duration::from_secs(30), stale)
        .await
        .expect("stale operation settles after rejected use")?
        .expect_err("post-return underlying handle unexpectedly remained active");
    assert!(
        stale_error
            .to_string()
            .contains("underlying-tool capability is no longer active"),
        "unexpected stale-handle failure: {stale_error}"
    );
    assert_eq!(
        leaf_effect_count(&fixture),
        1,
        "the post-return handle reached the leaf"
    );
    Ok(())
}

#[test]
#[timeout("3m")]
async fn universal_results_are_validated_and_valid_stream_transformation_survives(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = fixture(last_unique_id, deps, owner, provider).await?;
    for mode in ["wrong-root", "wrong-nested", "wrong-error"] {
        let effects_before = fixture.effects.lock().unwrap().len();
        let result = invoke_tool(
            &fixture,
            "validation-probe",
            vec![SchemaValue::String(mode.to_string())],
        )
        .await?;
        let AgentInvocationResult::ExternalTool { result: Err(error) } = result else {
            anyhow::bail!("{mode} unexpectedly succeeded: {result:?}");
        };
        assert!(
            matches!(
                error,
                golem_common::model::oplog::payload::types::SerializableToolRpcError::RemoteToolError(ref inner)
                    if matches!(inner.as_ref(), golem_common::model::oplog::payload::types::SerializableToolError::InvalidResult(_))
            ),
            "{mode} did not produce invalid-result: {error:?}"
        );
        assert_eq!(
            fixture.effects.lock().unwrap().len(),
            effects_before + 1,
            "{mode} repeated or skipped its independently observed leaf effect"
        );
        assert_entity_cleanup(&fixture).await;
    }

    let agent_id = agent_id!("ToolRuntimeBypassOwner", "e2-owner");
    let valid: ValidationEvidence = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.owner_component,
            &agent_id,
            "valid_transformation",
            crate::raw_params([]),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        valid,
        ValidationEvidence {
            label: "valid>valid-transform".to_string(),
            ordinal: 117,
            stdout: b"leaf-stream:valid-transform".to_vec(),
        }
    );
    assert_eq!(fixture.effects.lock().unwrap().len(), 4);
    assert_entity_cleanup(&fixture).await;

    let oplog = fixture
        .executor
        .get_oplog(&fixture.owner_id, OplogIndex::INITIAL)
        .await?;
    for start in oplog.iter().filter_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke" => {
            Some(entry.oplog_index)
        }
        _ => None,
    }) {
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)
                    || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start))
                .count(),
            1,
            "entity invocation {start} leaked after result validation"
        );
    }
    Ok(())
}

async fn invoke_chain(
    fixture: &Fixture,
    claimed: &str,
    mode: &str,
) -> anyhow::Result<ChainEvidence> {
    invoke_chain_for(
        fixture,
        &fixture.owner_id,
        fixture.fingerprint,
        claimed,
        mode,
    )
    .await
}

async fn invoke_chain_for(
    fixture: &Fixture,
    owner_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    claimed: &str,
    mode: &str,
) -> anyhow::Result<ChainEvidence> {
    let result = invoke_tool_for(
        fixture,
        owner_id,
        fingerprint,
        "chain-probe",
        vec![
            SchemaValue::String(claimed.to_string()),
            SchemaValue::String(mode.to_string()),
            SchemaValue::String("payload-principal".to_string()),
        ],
    )
    .await?;
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = result else {
        anyhow::bail!("chain probe failed: {result:?}");
    };
    let value = result.result.expect("chain probe result");
    ChainEvidence::from_value(value.value()).map_err(anyhow::Error::msg)
}

fn leaf_effect_count(fixture: &Fixture) -> usize {
    fixture
        .effects
        .lock()
        .unwrap()
        .iter()
        .filter(|effect| effect.starts_with("chain/"))
        .count()
}

async fn assert_entity_cleanup(fixture: &Fixture) {
    let owner = OwnedAgentId::new(fixture.environment_id, &fixture.owner_id);
    if let Some(active) = fixture.executor.active_entity_metadata(&owner).await {
        assert!(
            active.tool_operations.operations.is_empty(),
            "tool operations leaked after result settlement: {active:#?}"
        );
        assert!(
            active.slots.iter().all(|slot| slot.invocations.is_empty()),
            "entity invocations leaked after result settlement: {active:#?}"
        );
        assert!(
            active.lane.holder.is_none() && active.lane.active_invocation_count == 0,
            "owner lane remained occupied after result settlement: {active:#?}"
        );
    }
}

async fn invoke_tool(
    fixture: &Fixture,
    name: &str,
    fields: Vec<SchemaValue>,
) -> anyhow::Result<AgentInvocationResult> {
    invoke_tool_for(
        fixture,
        &fixture.owner_id,
        fixture.fingerprint,
        name,
        fields,
    )
    .await
}

async fn invoke_tool_for(
    fixture: &Fixture,
    owner_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    name: &str,
    fields: Vec<SchemaValue>,
) -> anyhow::Result<AgentInvocationResult> {
    let command_path = vec![match name {
        "chain-probe" => "inspect".to_string(),
        "validation-probe" => "validate".to_string(),
        _ => unreachable!(),
    }];
    let name = ToolName::try_from(name).unwrap();
    let definition = &fixture.definitions[&name];
    let command = definition
        .command_index_by_path(&command_path)
        .ok_or_else(|| anyhow::anyhow!("tool `{name}` has no inspect command"))?;
    let schema = definition.canonical_input_record_schema(command)?;
    let environment_id = if owner_id == &fixture.cross_environment_owner_id {
        fixture.other_environment_id
    } else {
        fixture.environment_id
    };
    let output = fixture
        .executor
        .invoke_external_tool_in_environment(
            environment_id,
            owner_id,
            fingerprint,
            IdempotencyKey::fresh(),
            name,
            command_path,
            TypedSchemaValue::new(schema, SchemaValue::Record { fields }),
            InvocationContextStack::fresh(),
            Principal::GolemUser(GolemUserPrincipal {
                account_id: fixture.account_id,
            }),
            None,
        )
        .await?;
    Ok(output.result)
}

async fn start_effect_server() -> anyhow::Result<(
    u16,
    tokio_util::task::AbortOnDropHandle<()>,
    Arc<Mutex<Vec<String>>>,
)> {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = Router::new()
        .route("/{*path}", post(record_effect))
        .with_state(effects.clone());
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    Ok((port, server, effects))
}

async fn record_effect(
    State(effects): State<Arc<Mutex<Vec<String>>>>,
    Path(path): Path<String>,
) -> StatusCode {
    effects.lock().unwrap().push(path);
    StatusCode::NO_CONTENT
}

fn deployment_state(
    account_id: AccountId,
    provider_id: golem_common::model::component::ComponentId,
    provider_revision: ComponentRevision,
    definitions: Vec<golem_common::schema::tool::Tool>,
) -> ToolDeploymentState {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: AgentTypeName("ToolRuntimeBypassOwner".to_string()),
    };
    let email = AccountEmail::new("e2@golem");
    let registered_tools = definitions
        .into_iter()
        .map(|definition| {
            let name = ToolName::try_from(definition.commands.nodes[0].name.clone()).unwrap();
            (
                name,
                RegisteredTool {
                    deployment_revision,
                    release_id: None,
                    metadata_digest: Default::default(),
                    definition,
                    provision: ToolProvisionConfig {
                        env: BTreeMap::from([
                            ("OWNER_MARKER".to_string(), "publisher-config".to_string()),
                            ("EFFECT_PORT".to_string(), "1".to_string()),
                        ]),
                        ..Default::default()
                    },
                    component_bindings: Default::default(),
                    source: ToolSource::Component {
                        component_id: provider_id,
                        component_revision: provider_revision,
                        component_name: ComponentName(
                            "golem-it:tool-runtime-bypass-provider".to_string(),
                        ),
                    },
                    owner_account_id: account_id,
                    owner_account_email: email.clone(),
                    metadata_version: "0.1.0".to_string(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let bindings = registered_tools
        .iter()
        .map(|(name, tool)| {
            (
                name.clone(),
                CompiledToolBinding {
                    deployment_revision,
                    release_id: None,
                    metadata_digest: Default::default(),
                    owner: owner.clone(),
                    tool_name: name.clone(),
                    version: tool.definition.version.clone(),
                    metadata_version: tool.metadata_version.clone(),
                    account_id,
                    account_email: email.clone(),
                    parameters: NormalizedJsonValue::new(serde_json::json!({})),
                    config_keys_readable: golem_common::model::tool::ConfigKeyScope::All,
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: ToolFilesystemAccess::Unset,
                    source: tool.source.clone(),
                },
            )
        })
        .collect();
    ToolDeploymentState {
        deployment_revision,
        registered_tools,
        tool_bindings: BTreeMap::from([(owner, bindings)]),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: Default::default(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    }
}

fn install_chain(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    middleware_id: golem_common::model::component::ComponentId,
    middleware_revision: ComponentRevision,
    definitions: &[ToolMiddleware],
    names: &[&str],
) {
    let email = AccountEmail::new("middleware@golem");
    let account_id = deployment.registered_tools[tool_name].owner_account_id;
    let registrations = definitions
        .iter()
        .map(|definition| {
            let name = ToolMiddlewareName::try_from(definition.name.as_str()).unwrap();
            (
                name,
                RegisteredToolMiddleware {
                    deployment_revision: deployment.deployment_revision,
                    release_id: None,
                    definition: definition.clone(),
                    provision: ToolProvisionConfig::default(),
                    source: ToolMiddlewareSource::Component {
                        component_id: middleware_id,
                        component_revision: middleware_revision,
                        component_name: ComponentName(
                            "golem-it:tool-runtime-bypass-middleware".to_string(),
                        ),
                    },
                    owner_account_id: account_id,
                    owner_account_email: email.clone(),
                    metadata_version: "0.1.0".to_string(),
                    metadata_digest: Default::default(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut effective = deployment.registered_tools[tool_name].definition.clone();
    let mut occurrences = names
        .iter()
        .rev()
        .map(|name| {
            let name = ToolMiddlewareName::try_from(*name).unwrap();
            let middleware = registrations[&name].clone();
            let (expected_definition, presented_definition) = match &middleware.definition.scope {
                ToolMiddlewareScope::Universal => (None, None),
                ToolMiddlewareScope::Monomorphic(scope) => {
                    (scope.expected.clone(), Some(scope.presented.clone()))
                }
            };
            let next_effective_definition = effective.clone();
            let compatibility = expected_definition.as_ref().map(|expected| {
                golem_common::schema::tool::compatibility::compile_tool_compatibility(
                    expected,
                    &next_effective_definition,
                    golem_common::schema::tool::compatibility::ToolCompatibilityMode::StructuralSubtype,
                )
                .expect("E2 middleware compatibility")
            });
            if let Some(presented) = &presented_definition {
                effective = presented.clone();
            }
            CompiledToolMiddlewareOccurrence {
                parameters: TypedSchemaValue::new(
                    middleware.definition.parameter_schema.clone(),
                    SchemaValue::Record { fields: Vec::new() },
                ),
                middleware,
                provision: ToolProvisionConfig::default(),
                config_keys_readable: golem_common::model::tool::ConfigKeyScope::All,
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
                expected_definition,
                presented_definition,
                next_effective_definition,
                compatibility,
            }
        })
        .collect::<Vec<_>>();
    occurrences.reverse();
    deployment.registered_tool_middlewares.extend(registrations);
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    deployment
        .tool_middleware_chains
        .entry(owner.clone())
        .or_default()
        .insert(
            tool_name.clone(),
            CompiledToolMiddlewareChain {
                deployment_revision: deployment.deployment_revision,
                owner,
                tool_name: tool_name.clone(),
                effective_definition: effective,
                occurrences,
            },
        );
}
