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

//! Tool permission admission for middleware `Underlying` calls. The
//! `runtime-bypass-permission` universal middleware rewrites `permission-probe` calls according
//! to the outer path argument (see the fixture source), and reports the inner outcome as its
//! result: `ok:<leaf result>`, `denied`, or `error:<detail>`.

use super::{deployment_state, install_chain};
use crate::Tracing;
use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::agent::{AgentTypeName, GolemUserPrincipal, ParsedAgentId, Principal};
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::{PolymorphicToolOwnerPattern, ToolOwnerPattern};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, ClassPermissionPattern, PermissionPattern,
    PolymorphicCard, PolymorphicClassPermissionPattern, PolymorphicPermissionPattern, ScopeCard,
    StoredCard, ToolInvocationPattern, ToolResourcePattern, ToolVerb,
};
use golem_common::model::component::{ComponentDto, ComponentId, ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::mcp_import::McpImportSource;
use golem_common::model::oplog::payload::types::SerializableToolRpcError;
use golem_common::model::oplog::{
    OplogIndex, PublicOplogEntry, PublicOplogEntryAttribution, PublicOplogEntryWithIndex,
};
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::tool::{ToolBindingOwner, ToolDeploymentState, ToolName};
use golem_common::model::{AgentInvocationResult, IdempotencyKey, PromiseId};
use golem_common::schema::tool::DiscoveredTool;
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::clients::registry::{McpRuntimeCredential, RegistryServiceError};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::mcp_import::McpImportObservation;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{CardService, CardState};
use golem_worker_executor::services::environment_state::{
    EnvironmentStateService, ToolActivationOutcome, ToolDiscoveryError,
};
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestCardService, TestContext, TestExecutorOverrides,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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

const OWNER_AGENT_TYPE: &str = "ToolRuntimeBypassOwner";
const PERMISSION_PROBE: &str = "permission-probe";
const PERMISSION_MIDDLEWARE: &str = "runtime-bypass-permission";
const MIDDLEWARE_COMPONENT: &str = "golem-it:tool-runtime-bypass-middleware";

/// Card registry for the owner. Registered cards are live with their registered payload, other
/// cards are live as in [`TestCardService`], revoked cards report `Revoked`, and one
/// `check_cards` call naming a gated card can be held until the test releases it.
#[derive(Default)]
struct PermissionCards {
    registered: Mutex<HashMap<CardId, StoredCard>>,
    revoked: Mutex<HashSet<CardId>>,
    gate: Mutex<Option<CheckCardsGate>>,
}

struct CheckCardsGate {
    card_id: CardId,
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

/// Observes that a held service call was entered; releasing it, or dropping the handle, lets the
/// call continue.
struct GateHandle {
    entered: Option<tokio::sync::oneshot::Receiver<()>>,
    release: Option<tokio::sync::oneshot::Sender<()>>,
}

impl GateHandle {
    async fn entered(&mut self) -> anyhow::Result<()> {
        let entered = self
            .entered
            .take()
            .ok_or_else(|| anyhow::anyhow!("the gate was already entered"))?;
        tokio::time::timeout(Duration::from_secs(30), entered)
            .await
            .map_err(|_| anyhow::anyhow!("no service call reached the gate"))?
            .map_err(|_| anyhow::anyhow!("the gate was dropped before it was entered"))
    }

    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

impl PermissionCards {
    fn register(&self, card: StoredCard) {
        self.registered.lock().unwrap().insert(card.card_id(), card);
    }

    fn revoke(&self, card_id: CardId) {
        self.revoked.lock().unwrap().insert(card_id);
    }

    /// Holds the next card check naming `card_id`.
    fn hold_check(&self, card_id: CardId) -> GateHandle {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self.gate.lock().unwrap() = Some(CheckCardsGate {
            card_id,
            entered: entered_tx,
            release: release_rx,
        });
        GateHandle {
            entered: Some(entered_rx),
            release: Some(release_tx),
        }
    }

    async fn pass_gate(&self, card_ids: &[CardId]) {
        let gate = {
            let mut slot = self.gate.lock().unwrap();
            match slot.as_ref() {
                Some(gate) if card_ids.contains(&gate.card_id) => slot.take(),
                _ => None,
            }
        };
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate.release.await;
        }
    }
}

struct PermissionCardService {
    cards: Arc<PermissionCards>,
}

#[async_trait]
impl CardService for PermissionCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        for card_id in card_ids {
            self.cards.revoke(*card_id);
        }
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        self.cards.pass_gate(&card_ids).await;
        let mut states = TestCardService.check_cards(card_ids).await?;
        let registered = self.cards.registered.lock().unwrap();
        let revoked = self.cards.revoked.lock().unwrap();
        for (card_id, state) in states.iter_mut() {
            if revoked.contains(card_id) {
                *state = CardState::Revoked;
            } else if let Some(card) = registered.get(card_id) {
                *state = CardState::Live(Box::new(card.clone()));
            }
        }
        Ok(states)
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        provenance: CardManagedByRuntimeDerived,
    ) -> Result<StoredCard, WorkerExecutorError> {
        TestCardService.create_runtime_card(card, provenance).await
    }
}

/// Holds the first live tool deployment lookup that follows an `ambient-starting` effect. The
/// effect server arms the gate before it answers, so the guest cannot reach its next host call
/// before the gate is armed.
#[derive(Default)]
struct DeploymentLookupGate {
    armed: AtomicBool,
    hold: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl DeploymentLookupGate {
    fn hold_after_ambient_start(&self) -> GateHandle {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self.hold.lock().unwrap() = Some((entered_tx, release_rx));
        GateHandle {
            entered: Some(entered_rx),
            release: Some(release_tx),
        }
    }

    fn on_effect(&self, path: &str) {
        if path == "ambient-starting" && self.hold.lock().unwrap().is_some() {
            self.armed.store(true, Ordering::SeqCst);
        }
    }

    async fn pass(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        let hold = self.hold.lock().unwrap().take();
        if let Some((entered, release)) = hold {
            let _ = entered.send(());
            let _ = release.await;
        }
    }
}

/// [`TestEnvironmentStateService`] with a [`DeploymentLookupGate`] on live tool deployment
/// lookups, which every ambient tool admission performs to resolve its activation.
struct GatedEnvironmentStateService {
    inner: Arc<TestEnvironmentStateService>,
    gate: Arc<DeploymentLookupGate>,
}

#[async_trait]
impl EnvironmentStateService for GatedEnvironmentStateService {
    async fn get_agent_deployment(
        &self,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        self.inner
            .get_agent_deployment(environment_id, agent_type)
            .await
    }

    async fn get_agent_secrets(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<HashMap<CanonicalAgentSecretPath, AgentSecret>, WorkerExecutorError> {
        self.inner.get_agent_secrets(environment_id).await
    }

    async fn get_agent_secret_revision(
        &self,
        environment_id: EnvironmentId,
        agent_secret_id: AgentSecretId,
        path: CanonicalAgentSecretPath,
        revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, WorkerExecutorError> {
        self.inner
            .get_agent_secret_revision(environment_id, agent_secret_id, path, revision)
            .await
    }

    async fn get_retry_policies(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<NamedRetryPolicy>, WorkerExecutorError> {
        self.inner.get_retry_policies(environment_id).await
    }

    async fn get_live_tool_deployment_state(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<Arc<ToolDeploymentState>>, ToolDiscoveryError> {
        self.gate.pass().await;
        self.inner
            .get_live_tool_deployment_state(environment_id, component_id, component_revision)
            .await
    }

    async fn resolve_mcp_import(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
        refresh: bool,
    ) -> Result<McpImportObservation, RegistryServiceError> {
        self.inner.resolve_mcp_import(source, auth, refresh).await
    }

    async fn get_mcp_runtime_credential(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<McpRuntimeCredential, RegistryServiceError> {
        self.inner.get_mcp_runtime_credential(source, auth).await
    }

    async fn report_mcp_resource_unauthorized(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
        generation: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        self.inner
            .report_mcp_resource_unauthorized(source, auth, generation)
            .await
    }

    async fn get_tool_deployment_state_at_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) -> Result<Arc<ToolDeploymentState>, ToolDiscoveryError> {
        self.inner
            .get_tool_deployment_state_at_revision(environment_id, deployment_revision)
            .await
    }

    async fn get_tool_activation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
        tool_name: &ToolName,
    ) -> Result<ToolActivationOutcome, ToolDiscoveryError> {
        self.inner
            .get_tool_activation(
                environment_id,
                component_id,
                component_revision,
                owner,
                tool_name,
            )
            .await
    }

    async fn get_accessible_tools(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
    ) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        self.inner
            .get_accessible_tools(environment_id, component_id, component_revision, owner)
            .await
    }

    async fn get_accessible_tool(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        owner: &ToolBindingOwner,
        tool_name: &ToolName,
    ) -> Result<Option<Arc<DiscoveredTool>>, ToolDiscoveryError> {
        self.inner
            .get_accessible_tool(
                environment_id,
                component_id,
                component_revision,
                owner,
                tool_name,
            )
            .await
    }

    async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        self.inner.invalidate_environment(environment_id).await
    }

    async fn invalidate_all(&self) {
        self.inner.invalidate_all().await
    }
}

#[derive(Clone)]
struct EffectRecorder {
    effects: Arc<Mutex<Vec<String>>>,
    deployment_gate: Arc<DeploymentLookupGate>,
}

async fn start_effect_server(
    deployment_gate: Arc<DeploymentLookupGate>,
) -> anyhow::Result<(
    u16,
    tokio_util::task::AbortOnDropHandle<()>,
    Arc<Mutex<Vec<String>>>,
)> {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = Router::new()
        .route("/{*path}", post(record_effect))
        .with_state(EffectRecorder {
            effects: effects.clone(),
            deployment_gate,
        });
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    Ok((port, server, effects))
}

async fn record_effect(
    State(recorder): State<EffectRecorder>,
    Path(path): Path<String>,
) -> StatusCode {
    recorder.deployment_gate.on_effect(&path);
    recorder.effects.lock().unwrap().push(path);
    StatusCode::NO_CONTENT
}

struct PermissionFixture {
    executor: TestWorkerExecutor,
    owner_component: ComponentDto,
    owner_agent: ParsedAgentId,
    owner_id: golem_common::model::AgentId,
    environment_id: EnvironmentId,
    account_id: AccountId,
    fingerprint: golem_common::model::AgentFingerprint,
    probe: golem_common::schema::tool::Tool,
    effects: Arc<Mutex<Vec<String>>>,
    cards: Arc<PermissionCards>,
    deployment_gate: Arc<DeploymentLookupGate>,
    _effect_server: tokio_util::task::AbortOnDropHandle<()>,
}

/// Starts an owner whose initial card keeps every default grant except the Tool class, and adds
/// `tool_grants`. Its `permission-probe` calls pass through the `runtime-bypass-permission`
/// middleware.
async fn permission_fixture(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    owner: &PrecompiledComponent,
    provider: &PrecompiledComponent,
    tool_grants: Vec<PolymorphicPermissionPattern>,
) -> anyhow::Result<PermissionFixture> {
    let context = TestContext::new(last_unique_id);
    let publisher_environment_id = EnvironmentId::new();
    let environment = Arc::new(TestEnvironmentStateService::default());
    let cards = Arc::new(PermissionCards::default());
    let deployment_gate = Arc::new(DeploymentLookupGate::default());
    let (effect_port, effect_server, effects) =
        start_effect_server(deployment_gate.clone()).await?;
    environment.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["contextSecret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: golem_common::schema::SchemaGraph::anonymous(
            golem_common::schema::SchemaType::string(),
        ),
        secret_value: Some(SchemaValue::String("caller-secret".to_string())),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(Arc::new(GatedEnvironmentStateService {
                inner: environment.clone(),
                gate: deployment_gate.clone(),
            })),
            create_card_service: Some(Arc::new({
                let cards = cards.clone();
                move || {
                    Arc::new(PermissionCardService {
                        cards: cards.clone(),
                    })
                }
            })),
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
        .update_agent_provision_config(OWNER_AGENT_TYPE, |config| {
            let positive = &mut config.initial_permissions.lower_bound.positive;
            positive.retain(|grant| !matches!(grant, PolymorphicPermissionPattern::Tool(_)));
            positive.extend(tool_grants);
        })
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &publisher_environment_id,
            "golem_it_tool_runtime_bypass_middleware_release",
        )
        .name(MIDDLEWARE_COMPONENT)
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
    let probe = provider_metadata
        .tools
        .iter()
        .find(|definition| definition.commands.nodes[0].name == PERMISSION_PROBE)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("the provider does not define {PERMISSION_PROBE}"))?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        provider_metadata.tools.clone(),
    );
    install_chain(
        &mut deployment,
        &AgentTypeName(OWNER_AGENT_TYPE.to_string()),
        &ToolName::try_from(PERMISSION_PROBE).unwrap(),
        middleware_component.id,
        middleware_component.revision,
        &middleware_metadata.tool_middlewares,
        &[PERMISSION_MIDDLEWARE],
    );
    environment.set_tool_deployment(
        context.default_environment_id,
        owner_component.id,
        owner_component.revision,
        Some(deployment),
    );
    let owner_agent = agent_id!("ToolRuntimeBypassOwner", "permission-owner");
    let owner_id = executor
        .start_agent_with(
            &owner_component.id,
            owner_agent.clone(),
            HashMap::from([
                ("OWNER_MARKER".to_string(), "caller-config".to_string()),
                ("EFFECT_PORT".to_string(), effect_port.to_string()),
            ]),
            Vec::new(),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&owner_id).await?.fingerprint;
    Ok(PermissionFixture {
        executor,
        owner_component,
        owner_agent,
        owner_id,
        environment_id: context.default_environment_id,
        account_id: context.account_id,
        fingerprint,
        probe,
        effects,
        cards,
        deployment_gate,
        _effect_server: effect_server,
    })
}

fn probe_invocation(command: &str, path: &str) -> ToolResourcePattern {
    ToolResourcePattern::Invocation(
        ToolInvocationPattern::from_command_and_args(&[command], &[&format!("{path:?}")])
            .expect("valid permission-probe invocation"),
    )
}

/// Grants `<command> "<path>"` on the caller component's `permission-probe`.
fn owner_grant(command: &str, path: &str) -> PolymorphicPermissionPattern {
    PolymorphicPermissionPattern::Tool(PolymorphicClassPermissionPattern {
        verb: Some(ToolVerb::Invoke),
        owner: PolymorphicToolOwnerPattern::ComponentTool {
            tool: PERMISSION_PROBE.to_string(),
        },
        recipient: RecipientPattern::Any,
        resource: probe_invocation(command, path),
    })
}

/// Grants `<command> "<path>"` on a `permission-probe` owned by the middleware component.
fn middleware_component_grant(command: &str, path: &str) -> PolymorphicPermissionPattern {
    PolymorphicPermissionPattern::Tool(PolymorphicClassPermissionPattern {
        verb: Some(ToolVerb::Invoke),
        owner: PolymorphicToolOwnerPattern::EnvTool {
            component: ComponentName(MIDDLEWARE_COMPONENT.to_string()),
            tool: PERMISSION_PROBE.to_string(),
        },
        recipient: RecipientPattern::Any,
        resource: probe_invocation(command, path),
    })
}

/// The scope-card form of [`owner_grant`]: scope cards carry concrete owners.
fn concrete_owner_grant(
    fixture: &PermissionFixture,
    command: &str,
    path: &str,
) -> PermissionPattern {
    PermissionPattern::Tool(ClassPermissionPattern {
        verb: Some(ToolVerb::Invoke),
        owner: ToolOwnerPattern::Tool {
            account: AccountEmail::new("test@golem"),
            application: ApplicationName::try_from("test-app".to_string()).unwrap(),
            environment: EnvironmentName::try_from("test-env").unwrap(),
            component: fixture.owner_component.component_name.clone(),
            tool: PERMISSION_PROBE.to_string(),
        },
        recipient: RecipientPattern::Any,
        resource: probe_invocation(command, path),
    })
}

fn tool_card(grants: Vec<PolymorphicPermissionPattern>) -> StoredCard {
    StoredCard::Polymorphic(PolymorphicCard {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: grants,
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
    })
}

/// Registers `card` with the card registry and queues its installation into the owner's wallet.
async fn install_card(fixture: &PermissionFixture, card: StoredCard) -> anyhow::Result<()> {
    fixture.cards.register(card.clone());
    fixture
        .executor
        .queue_card_install(&fixture.owner_id, card)
        .await
}

/// Revokes `card_id` in the card registry and queues its revocation for the owner.
async fn revoke_card(fixture: &PermissionFixture, card_id: CardId) -> anyhow::Result<()> {
    fixture.cards.revoke(card_id);
    fixture
        .executor
        .queue_card_revocation(&fixture.owner_id, card_id)
        .await
}

fn scope_card(
    root: CardId,
    lower_positive: Vec<PermissionPattern>,
    upper_negative: Vec<PermissionPattern>,
) -> ScopeCard {
    ScopeCard {
        scope_card_id: CardId::new(),
        root_card_ids: vec![root],
        lower_positive,
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The outer call completed with this middleware result.
    Result(String),
    /// The outer call itself was denied.
    Denied,
    Other(String),
}

fn ok(result: &str) -> Outcome {
    Outcome::Result(format!("ok:{result}"))
}

fn inner_denied() -> Outcome {
    Outcome::Result("denied".to_string())
}

async fn probe(
    fixture: &PermissionFixture,
    key: &IdempotencyKey,
    command: &str,
    path: &str,
    scope: Option<ScopeCard>,
) -> anyhow::Result<Outcome> {
    let command_path = vec![command.to_string()];
    let index = fixture
        .probe
        .command_index_by_path(&command_path)
        .ok_or_else(|| anyhow::anyhow!("{PERMISSION_PROBE} has no {command} command"))?;
    let schema = fixture.probe.canonical_input_record_schema(index)?;
    let output = fixture
        .executor
        .invoke_external_tool_in_environment(
            fixture.environment_id,
            &fixture.owner_id,
            fixture.fingerprint,
            key.clone(),
            ToolName::try_from(PERMISSION_PROBE).unwrap(),
            command_path,
            TypedSchemaValue::new(
                schema,
                SchemaValue::Record {
                    fields: vec![SchemaValue::String(path.to_string())],
                },
            ),
            InvocationContextStack::fresh(),
            Principal::GolemUser(GolemUserPrincipal {
                account_id: fixture.account_id,
            }),
            scope,
        )
        .await?;
    Ok(match output.result {
        AgentInvocationResult::ExternalTool { result: Ok(result) } => {
            match result.result.as_ref().map(|value| value.value()) {
                Some(SchemaValue::String(value)) => Outcome::Result(value.clone()),
                other => Outcome::Other(format!("unexpected result value: {other:?}")),
            }
        }
        AgentInvocationResult::ExternalTool {
            result: Err(SerializableToolRpcError::Denied(_)),
        } => Outcome::Denied,
        other => Outcome::Other(format!("{other:?}")),
    })
}

async fn probe_fresh(
    fixture: &PermissionFixture,
    command: &str,
    path: &str,
    scope: Option<ScopeCard>,
) -> anyhow::Result<Outcome> {
    probe(fixture, &IdempotencyKey::fresh(), command, path, scope).await
}

fn permission_effects(fixture: &PermissionFixture) -> Vec<String> {
    fixture
        .effects
        .lock()
        .unwrap()
        .iter()
        .filter(|effect| effect.starts_with("permission/"))
        .cloned()
        .collect()
}

async fn owner_oplog(
    fixture: &PermissionFixture,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    fixture
        .executor
        .get_oplog(&fixture.owner_id, OplogIndex::INITIAL)
        .await
}

fn entity_starts<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    name: &'a str,
) -> impl Iterator<Item = OplogIndex> + 'a {
    oplog
        .iter()
        .filter_map(move |entry| match (&entry.entry, &entry.attribution) {
            (PublicOplogEntry::Start(start), PublicOplogEntryAttribution::Entity(entity))
                if start.function_name == "golem::entity::invoke"
                    && entity.invocation.entity.name == name =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
}

const ENTITY_INVOKE: &str = "golem::entity::invoke";

fn is_invocation_rejected(function_name: &str) -> bool {
    function_name.ends_with("invocation-rejected")
}

/// The durable records of tool calls rejected at admission.
fn rejection_starts(oplog: &[PublicOplogEntryWithIndex]) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if is_invocation_rejected(&start.function_name) => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

fn terminal_count(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)
                || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start)
        })
        .count()
}

/// Returns the entity invocation and invocation-rejected `Start`s that have no `End` or
/// `Cancelled`, and fails when one has more than one terminal.
fn unterminated_entity_starts(oplog: &[PublicOplogEntryWithIndex]) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == ENTITY_INVOKE
                    || is_invocation_rejected(&start.function_name) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .filter(|start| {
            let terminals = terminal_count(oplog, *start);
            assert!(
                terminals <= 1,
                "entity invocation {start} has {terminals} terminals"
            );
            terminals == 0
        })
        .collect()
}

/// Fails when a positional record (`Start`, `End` or `Cancelled`) follows the last
/// `AgentInvocationFinished` before the next invocation starts.
fn assert_no_positional_records_after_last_finished(oplog: &[PublicOplogEntryWithIndex]) {
    let Some(finished) = oplog
        .iter()
        .rposition(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
    else {
        panic!("no invocation has finished");
    };
    for entry in oplog[finished + 1..]
        .iter()
        .take_while(|entry| !matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
    {
        assert!(
            !matches!(
                entry.entry,
                PublicOplogEntry::Start(_)
                    | PublicOplogEntry::End(_)
                    | PublicOplogEntry::Cancelled(_)
            ),
            "positional record {} follows AgentInvocationFinished at {}: {:?}",
            entry.oplog_index,
            oplog[finished].oplog_index,
            entry.entry
        );
    }
}

async fn wait_for_effect(fixture: &PermissionFixture, prefix: &str) -> String {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(effect) = fixture
                .effects
                .lock()
                .unwrap()
                .iter()
                .find_map(|effect| effect.strip_prefix(prefix).map(str::to_string))
            {
                break effect;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no `{prefix}` effect was recorded"))
}

/// An invocation key with the request it was recorded for and its recorded outcome.
type RecordedCall = (
    IdempotencyKey,
    &'static str,
    &'static str,
    Option<ScopeCard>,
    Outcome,
);

/// Public oplog entry content without the rendering order of span attributes, which the public
/// oplog does not preserve across a reload.
fn comparable(entry: &PublicOplogEntryWithIndex) -> serde_json::Value {
    fn normalize(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(normalize);
                if items.iter().all(|item| {
                    item.as_object()
                        .is_some_and(|item| item.contains_key("key"))
                }) {
                    items.sort_by_key(|item| item.to_string());
                }
            }
            serde_json::Value::Object(fields) => fields.values_mut().for_each(normalize),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(entry).expect("public oplog entries serialize");
    normalize(&mut value);
    value
}

/// The leaf effects and owner oplog recorded before a simulated crash.
struct HistoryBeforeCrash {
    effects: Vec<String>,
    oplog: Vec<PublicOplogEntryWithIndex>,
}

/// Crashes the owner and recovers it from its oplog.
async fn crash_and_recover(fixture: &PermissionFixture) -> anyhow::Result<HistoryBeforeCrash> {
    let history = HistoryBeforeCrash {
        effects: permission_effects(fixture),
        oplog: owner_oplog(fixture).await?,
    };
    fixture.executor.simulated_crash(&fixture.owner_id).await?;
    fixture.executor.resume(&fixture.owner_id, true).await?;
    Ok(history)
}

impl HistoryBeforeCrash {
    /// Checks that replay neither repeated nor added a leaf effect or leaf invocation, and did
    /// not rewrite the history recorded before the crash.
    async fn assert_preserved(&self, fixture: &PermissionFixture) -> anyhow::Result<()> {
        assert_eq!(
            permission_effects(fixture),
            self.effects,
            "replaying recorded tool admissions repeated or added a leaf effect"
        );
        let oplog = owner_oplog(fixture).await?;
        if let Some((before, after)) = self
            .oplog
            .iter()
            .zip(oplog.iter())
            .find(|(before, after)| comparable(before) != comparable(after))
        {
            panic!("the restart rewrote recorded history:\nbefore: {before:#?}\nafter: {after:#?}");
        }
        assert!(
            oplog.len() >= self.oplog.len(),
            "the restart dropped recorded history"
        );
        assert_eq!(
            entity_starts(&oplog, PERMISSION_PROBE).count(),
            entity_starts(&self.oplog, PERMISSION_PROBE).count(),
            "the restart started a new leaf invocation for a recorded call"
        );
        assert_eq!(unterminated_entity_starts(&oplog), Vec::new());
        Ok(())
    }
}

/// Crashes the owner, recovers it from its oplog, and checks that every invocation recorded
/// before the crash returns its recorded outcome without changing the recorded history.
async fn assert_recorded_outcomes_survive_restart(
    fixture: &PermissionFixture,
    recorded: &[RecordedCall],
) -> anyhow::Result<()> {
    let history = crash_and_recover(fixture).await?;
    for (key, command, path, scope, outcome) in recorded {
        assert_eq!(
            &probe(fixture, key, command, path, scope.clone()).await?,
            outcome,
            "the recorded outcome of `{command} {path}` changed after the restart"
        );
    }
    history.assert_preserved(fixture).await
}

/// Waits until the invocation has finished and every entity invocation and rejection record in
/// the owner oplog has a terminal. Then checks that the `permission-probe read /public` started
/// while its admission was pending was either rejected with a durable invocation-rejected record,
/// or admitted and settled with exactly one leaf effect, and that nothing positional was recorded
/// after the invocation finished.
async fn assert_pending_admission_settled(
    fixture: &PermissionFixture,
    finished_invocations_before: usize,
) -> anyhow::Result<()> {
    let settled = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = owner_oplog(fixture).await?;
            let finished = oplog
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
                .count();
            if finished > finished_invocations_before
                && unterminated_entity_starts(&oplog).is_empty()
            {
                break Ok::<_, anyhow::Error>(oplog);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the invocation and every entity invocation settle")?;
    match entity_starts(&settled, PERMISSION_PROBE).count() {
        0 => {
            assert_eq!(
                rejection_starts(&settled).len(),
                1,
                "a pending admission that never reached the leaf must leave a durable rejection"
            );
            assert!(
                permission_effects(fixture).is_empty(),
                "a rejected admission reached the provider"
            );
        }
        1 => {
            assert_eq!(
                permission_effects(fixture),
                vec!["permission/read/public"],
                "an admitted call must settle with exactly one leaf effect"
            );
        }
        leaf_starts => panic!("one pending call started {leaf_starts} leaf invocations"),
    }
    assert_no_positional_records_after_last_finished(&settled);
    Ok(())
}

/// Waits for a `wasi:logging` line containing `marker` from the owner.
async fn wait_for_log_marker(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<
        golem_api_grpc::proto::golem::worker::LogEvent,
    >,
    marker: &str,
) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            if format!("{event:?}").contains(marker) {
                return;
            }
        }
        panic!("the owner log stream ended before `{marker}`");
    })
    .await
    .unwrap_or_else(|_| panic!("the owner did not log `{marker}`"))
}

/// Checks, while an admission is held, that the guest has passed its last import before
/// returning but the owner has neither settled the entity invocations nor finished the
/// invocation.
async fn assert_unsettled_while_admission_is_held(
    fixture: &PermissionFixture,
    finished_invocations_before: usize,
    unterminated: &[&str],
) -> anyhow::Result<()> {
    let oplog = owner_oplog(fixture).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count(),
        finished_invocations_before,
        "the invocation finished while one of its admissions was still pending"
    );
    for name in unterminated {
        let starts = entity_starts(&oplog, name).collect::<Vec<_>>();
        assert_eq!(starts.len(), 1, "expected one `{name}` invocation");
        assert_eq!(
            terminal_count(&oplog, starts[0]),
            0,
            "`{name}` settled while its Underlying admission was still pending"
        );
    }
    assert_eq!(entity_starts(&oplog, PERMISSION_PROBE).count(), 0);
    assert!(rejection_starts(&oplog).is_empty());
    Ok(())
}

#[test]
#[timeout("3m")]
async fn underlying_admission_denies_middleware_rewritten_command_and_argument(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = permission_fixture(
        last_unique_id,
        deps,
        owner,
        provider,
        vec![
            owner_grant("read", "/public"),
            owner_grant("read", "/as-delete/public"),
            owner_grant("read", "/alias/secret"),
        ],
    )
    .await?;

    assert_eq!(
        probe_fresh(&fixture, "read", "/public", None).await?,
        ok("read:/public"),
        "a pass-through call whose inner representation is granted must be admitted"
    );
    assert_eq!(permission_effects(&fixture), vec!["permission/read/public"]);

    assert_eq!(
        probe_fresh(&fixture, "read", "/as-delete/public", None).await?,
        inner_denied(),
        "a granted outer command rewritten into a non-granted command must be denied"
    );
    assert_eq!(
        probe_fresh(&fixture, "read", "/alias/secret", None).await?,
        inner_denied(),
        "a granted outer argument rewritten into a non-granted argument must be denied"
    );
    assert_eq!(
        permission_effects(&fixture),
        vec!["permission/read/public"],
        "a denied Underlying call reached the provider"
    );

    let oplog = owner_oplog(&fixture).await?;
    assert_eq!(
        entity_starts(&oplog, PERMISSION_MIDDLEWARE).count(),
        3,
        "every outer call must enter the middleware"
    );
    assert_eq!(
        entity_starts(&oplog, PERMISSION_PROBE).count(),
        1,
        "a denied Underlying call started the provider"
    );
    assert_eq!(unterminated_entity_starts(&oplog), Vec::new());
    Ok(())
}

#[test]
#[timeout("3m")]
async fn adapter_needs_caller_owned_grants_for_outer_and_inner_representations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = permission_fixture(
        last_unique_id,
        deps,
        owner,
        provider,
        vec![owner_grant("read", "/alias/public")],
    )
    .await?;

    assert_eq!(
        probe_fresh(&fixture, "read", "/alias/public", None).await?,
        inner_denied(),
        "the outer grant alone must not admit the adapted inner call"
    );

    install_card(
        &fixture,
        tool_card(vec![middleware_component_grant("read", "/public")]),
    )
    .await?;
    assert_eq!(
        probe_fresh(&fixture, "read", "/alias/public", None).await?,
        inner_denied(),
        "a grant naming the middleware component must not admit the inner call"
    );
    assert!(permission_effects(&fixture).is_empty());

    install_card(&fixture, tool_card(vec![owner_grant("read", "/public")])).await?;
    assert_eq!(
        probe_fresh(&fixture, "read", "/alias/public", None).await?,
        ok("read:/public"),
        "grants for both the outer and the inner representation must admit the adapter"
    );
    assert_eq!(permission_effects(&fixture), vec!["permission/read/public"]);
    let oplog = owner_oplog(&fixture).await?;
    assert_eq!(entity_starts(&oplog, PERMISSION_PROBE).count(), 1);
    assert_eq!(unterminated_entity_starts(&oplog), Vec::new());
    Ok(())
}

#[test]
#[timeout("3m")]
async fn underlying_admission_composes_the_inherited_invocation_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = permission_fixture(last_unique_id, deps, owner, provider, Vec::new()).await?;

    assert_eq!(
        probe_fresh(&fixture, "read", "/alias/public", None).await?,
        Outcome::Denied,
        "without a scope the owner holds no Tool grant"
    );
    let scope_lower = vec![
        concrete_owner_grant(&fixture, "read", "/alias/public"),
        concrete_owner_grant(&fixture, "read", "/public"),
    ];
    assert_eq!(
        probe_fresh(
            &fixture,
            "read",
            "/alias/public",
            Some(scope_card(CardId::new(), scope_lower, Vec::new())),
        )
        .await?,
        ok("read:/public"),
        "scope lower grants must authorize the Underlying call in the middleware Store"
    );
    assert_eq!(
        probe_fresh(
            &fixture,
            "read",
            "/alias/public",
            Some(scope_card(
                CardId::new(),
                vec![concrete_owner_grant(&fixture, "read", "/alias/public")],
                Vec::new(),
            )),
        )
        .await?,
        inner_denied(),
        "a scope that grants only the outer call must not admit the inner call"
    );
    assert_eq!(permission_effects(&fixture), vec!["permission/read/public"]);

    install_card(
        &fixture,
        tool_card(vec![
            owner_grant("read", "/as-delete/public"),
            owner_grant("delete", "/public"),
        ]),
    )
    .await?;
    assert_eq!(
        probe_fresh(&fixture, "read", "/as-delete/public", None).await?,
        ok("delete:/public"),
        "the wallet alone admits both representations"
    );
    assert_eq!(
        probe_fresh(
            &fixture,
            "read",
            "/as-delete/public",
            Some(scope_card(
                CardId::new(),
                Vec::new(),
                vec![concrete_owner_grant(&fixture, "delete", "/public")],
            )),
        )
        .await?,
        inner_denied(),
        "a scope upper restriction must deny the rewritten inner call that the wallet allows"
    );
    assert_eq!(
        permission_effects(&fixture),
        vec!["permission/read/public", "permission/delete/public"]
    );
    assert_eq!(
        unterminated_entity_starts(&owner_oplog(&fixture).await?),
        Vec::new()
    );
    Ok(())
}

/// Runs `/twice/public`: the middleware calls `read /public`, waits at a promise checkpoint
/// while the test revokes `revoked_card`, then calls `read /public` again.
async fn revocation_between_underlying_calls(
    fixture: Arc<PermissionFixture>,
    scope: Option<ScopeCard>,
    revoked_card: CardId,
) -> anyhow::Result<IdempotencyKey> {
    let key = IdempotencyKey::fresh();
    let call = tokio::spawn({
        let fixture = fixture.clone();
        let key = key.clone();
        async move { probe(&fixture, &key, "read", "/twice/public", scope).await }
    });
    let checkpoint = wait_for_effect(&fixture, "permission-ready/").await;
    assert_eq!(permission_effects(&fixture), vec!["permission/read/public"]);

    revoke_card(&fixture, revoked_card).await?;
    fixture
        .executor
        .complete_promise(
            &PromiseId {
                agent_id: fixture.owner_id.clone(),
                oplog_idx: OplogIndex::from_u64(checkpoint.parse()?),
            },
            Vec::new(),
        )
        .await?;
    let outcome = tokio::time::timeout(Duration::from_secs(60), call)
        .await
        .expect("the revoked call settles")??;
    assert_eq!(
        outcome,
        Outcome::Result("first=ok:read:/public;second=denied".to_string()),
        "the admission after the revocation must be denied"
    );
    assert_eq!(
        permission_effects(&fixture),
        vec!["permission/read/public"],
        "the denied admission reached the provider"
    );
    let oplog = owner_oplog(&fixture).await?;
    assert_eq!(entity_starts(&oplog, PERMISSION_PROBE).count(), 1);
    assert_eq!(unterminated_entity_starts(&oplog), Vec::new());
    Ok(key)
}

#[test]
#[timeout("3m")]
async fn scope_root_revoked_between_underlying_calls_denies_the_later_admission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture =
        Arc::new(permission_fixture(last_unique_id, deps, owner, provider, Vec::new()).await?);
    let root = CardId::new();
    let scope = scope_card(
        root,
        vec![
            concrete_owner_grant(&fixture, "read", "/twice/public"),
            concrete_owner_grant(&fixture, "read", "/public"),
        ],
        Vec::new(),
    );
    let key =
        revocation_between_underlying_calls(fixture.clone(), Some(scope.clone()), root).await?;

    assert_recorded_outcomes_survive_restart(
        &fixture,
        &[(
            key,
            "read",
            "/twice/public",
            Some(scope),
            Outcome::Result("first=ok:read:/public;second=denied".to_string()),
        )],
    )
    .await?;
    let fresh_scope = scope_card(
        CardId::new(),
        vec![concrete_owner_grant(&fixture, "read", "/public")],
        Vec::new(),
    );
    assert_eq!(
        probe_fresh(&fixture, "read", "/public", Some(fresh_scope)).await?,
        ok("read:/public"),
        "a fresh scope must authorize new work after the restart"
    );
    Ok(())
}

#[test]
#[timeout("3m")]
async fn wallet_card_revoked_between_underlying_calls_denies_the_later_admission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture =
        Arc::new(permission_fixture(last_unique_id, deps, owner, provider, Vec::new()).await?);
    let card = tool_card(vec![
        owner_grant("read", "/twice/public"),
        owner_grant("read", "/public"),
    ]);
    let card_id = card.card_id();
    install_card(&fixture, card).await?;
    let key = revocation_between_underlying_calls(fixture.clone(), None, card_id).await?;

    assert_recorded_outcomes_survive_restart(
        &fixture,
        &[(
            key,
            "read",
            "/twice/public",
            None,
            Outcome::Result("first=ok:read:/public;second=denied".to_string()),
        )],
    )
    .await?;
    assert_eq!(
        probe_fresh(&fixture, "read", "/public", None).await?,
        Outcome::Denied,
        "the revoked wallet card must not authorize new work"
    );
    Ok(())
}

#[test]
#[timeout("3m")]
async fn recorded_admission_and_denials_replay_unchanged_after_grants_change(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = permission_fixture(last_unique_id, deps, owner, provider, Vec::new()).await?;
    let first_card = tool_card(vec![
        owner_grant("read", "/public"),
        owner_grant("read", "/as-delete/public"),
    ]);
    let first_card_id = first_card.card_id();
    install_card(&fixture, first_card).await?;

    let admitted = IdempotencyKey::fresh();
    let inner_denial = IdempotencyKey::fresh();
    let outer_denial = IdempotencyKey::fresh();
    let recorded: [RecordedCall; 3] = [
        (admitted, "read", "/public", None, ok("read:/public")),
        (
            inner_denial,
            "read",
            "/as-delete/public",
            None,
            inner_denied(),
        ),
        (outer_denial, "delete", "/public", None, Outcome::Denied),
    ];
    for (key, command, path, scope, outcome) in &recorded {
        assert_eq!(
            &probe(&fixture, key, command, path, scope.clone()).await?,
            outcome
        );
    }
    assert_eq!(permission_effects(&fixture), vec!["permission/read/public"]);

    // Under the changed grants each recorded call would get the opposite live decision.
    revoke_card(&fixture, first_card_id).await?;
    install_card(
        &fixture,
        tool_card(vec![
            owner_grant("read", "/as-delete/public"),
            owner_grant("delete", "/public"),
        ]),
    )
    .await?;

    assert_recorded_outcomes_survive_restart(&fixture, &recorded).await?;

    assert_eq!(
        probe_fresh(&fixture, "read", "/public", None).await?,
        Outcome::Denied,
        "new work must use the changed grants"
    );
    assert_eq!(
        probe_fresh(&fixture, "read", "/as-delete/public", None).await?,
        ok("delete:/public"),
        "new work must use the changed grants"
    );
    assert_eq!(
        permission_effects(&fixture),
        vec!["permission/read/public", "permission/delete/public"]
    );
    Ok(())
}

fn finished_invocations(oplog: &[PublicOplogEntryWithIndex]) -> usize {
    oplog
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .count()
}

#[test]
#[timeout("3m")]
async fn middleware_exit_during_pending_underlying_admission_records_the_rejection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = Arc::new(
        permission_fixture(
            last_unique_id,
            deps,
            owner,
            provider,
            vec![
                owner_grant("read", "/exit/public"),
                owner_grant("read", "/public"),
            ],
        )
        .await?,
    );
    let mut log = fixture.executor.capture_output(&fixture.owner_id).await?;
    let finished_before = finished_invocations(&owner_oplog(&fixture).await?);
    // While the middleware waits at its promise checkpoint, a card install is queued. The
    // Underlying admission the middleware starts after the promise completes is its first
    // authority boundary after the queued event, so it drains the install, and the registry
    // check of that card is held.
    let key = IdempotencyKey::fresh();
    let call = tokio::spawn({
        let fixture = fixture.clone();
        let key = key.clone();
        async move { probe(&fixture, &key, "read", "/exit/public", None).await }
    });
    let checkpoint = wait_for_effect(&fixture, "exit-ready/").await;
    let pending_card = tool_card(Vec::new());
    let mut gate = fixture.cards.hold_check(pending_card.card_id());
    install_card(&fixture, pending_card).await?;
    fixture
        .executor
        .complete_promise(
            &PromiseId {
                agent_id: fixture.owner_id.clone(),
                oplog_idx: OplogIndex::from_u64(checkpoint.parse()?),
            },
            Vec::new(),
        )
        .await?;
    gate.entered().await?;
    // The middleware started the call detached from its task. Its only remaining import before
    // `task.return` and exit is this log line; the held admission can only be polled again after
    // the guest yields, which it does by exiting.
    wait_for_log_marker(&mut log, "exit-returning").await;
    assert_unsettled_while_admission_is_held(&fixture, finished_before, &[PERMISSION_MIDDLEWARE])
        .await?;
    gate.release();

    let outcome = tokio::time::timeout(Duration::from_secs(60), call)
        .await
        .expect("the outer call settles")??;
    assert_eq!(
        outcome,
        Outcome::Result("returned-early".to_string()),
        "the middleware must exit while the Underlying admission is pending"
    );
    assert_pending_admission_settled(&fixture, finished_before).await?;

    assert_recorded_outcomes_survive_restart(
        &fixture,
        &[(
            key,
            "read",
            "/exit/public",
            None,
            Outcome::Result("returned-early".to_string()),
        )],
    )
    .await?;
    assert_eq!(
        probe_fresh(&fixture, "read", "/public", None).await?,
        ok("read:/public"),
        "the owner must keep serving new work after replaying the early exit"
    );
    Ok(())
}

#[test]
#[timeout("3m")]
async fn guest_exit_during_pending_ambient_invoke_and_await_admission_settles_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = Arc::new(
        permission_fixture(
            last_unique_id,
            deps,
            owner,
            provider,
            vec![owner_grant("read", "/public")],
        )
        .await?,
    );
    let mut log = fixture.executor.capture_output(&fixture.owner_id).await?;
    let finished_before = finished_invocations(&owner_oplog(&fixture).await?);
    // The owner method announces `ambient-starting` and then starts `invoke-and-await`; the
    // admission's activation lookup is the next live tool deployment lookup.
    let mut gate = fixture.deployment_gate.hold_after_ambient_start();
    let key = IdempotencyKey::fresh();
    let method = "start_permission_probe_and_exit";
    let call = tokio::spawn({
        let fixture = fixture.clone();
        let key = key.clone();
        async move {
            fixture
                .executor
                .invoke_and_await_agent_with_key(
                    &fixture.owner_component,
                    &fixture.owner_agent,
                    &key,
                    method,
                    data_value!("/public"),
                )
                .await?
                .into_typed::<String>()
        }
    });
    gate.entered().await?;
    // The owner started the call detached from its task. Its only remaining import before
    // `task.return` and exit is this log line; the held admission can only be polled again after
    // the guest yields, which it does by exiting.
    wait_for_log_marker(&mut log, "ambient-returning").await;
    assert_unsettled_while_admission_is_held(&fixture, finished_before, &[]).await?;
    assert_eq!(
        entity_starts(&owner_oplog(&fixture).await?, PERMISSION_MIDDLEWARE).count(),
        0,
        "the held lookup must precede the admission of the ambient call"
    );
    gate.release();

    let result = tokio::time::timeout(Duration::from_secs(60), call)
        .await
        .expect("the owner invocation settles")??;
    assert_eq!(
        result, "returned-early",
        "the guest must exit while the ambient admission is pending"
    );
    assert_pending_admission_settled(&fixture, finished_before).await?;

    let history = crash_and_recover(&fixture).await?;
    let replayed = fixture
        .executor
        .invoke_and_await_agent_with_key(
            &fixture.owner_component,
            &fixture.owner_agent,
            &key,
            method,
            data_value!("/public"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(replayed, result);
    history.assert_preserved(&fixture).await?;
    assert_eq!(
        probe_fresh(&fixture, "read", "/public", None).await?,
        ok("read:/public"),
        "the owner must keep serving new work after replaying the early exit"
    );
    Ok(())
}

#[test]
#[timeout("3m")]
async fn crash_during_an_admitted_call_repairs_it_and_denies_later_descendants(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture =
        Arc::new(permission_fixture(last_unique_id, deps, owner, provider, Vec::new()).await?);
    let root = CardId::new();
    let scope = scope_card(
        root,
        vec![
            concrete_owner_grant(&fixture, "read", "/recover/public"),
            concrete_owner_grant(&fixture, "read", "/held/public"),
            concrete_owner_grant(&fixture, "read", "/public"),
        ],
        Vec::new(),
    );
    let key = IdempotencyKey::fresh();
    let call = tokio::spawn({
        let fixture = fixture.clone();
        let key = key.clone();
        let scope = scope.clone();
        async move { probe(&fixture, &key, "read", "/recover/public", Some(scope)).await }
    });
    // The leaf of the first Underlying call performs its effect and then waits at a promise.
    let checkpoint = wait_for_effect(&fixture, "held-ready/").await;
    assert_eq!(
        permission_effects(&fixture),
        vec!["permission/read/held/public"]
    );
    let oplog = owner_oplog(&fixture).await?;
    let leaf = entity_starts(&oplog, PERMISSION_PROBE).collect::<Vec<_>>();
    assert_eq!(leaf.len(), 1, "the admitted leaf must have a durable Start");
    assert_eq!(terminal_count(&oplog, leaf[0]), 0);

    // Withdraw the inherited scope that admitted the leaf, then crash mid-execution.
    revoke_card(&fixture, root).await?;
    crash_and_recover(&fixture).await?;
    fixture
        .executor
        .complete_promise(
            &PromiseId {
                agent_id: fixture.owner_id.clone(),
                oplog_idx: OplogIndex::from_u64(checkpoint.parse()?),
            },
            Vec::new(),
        )
        .await?;
    let outcome = match tokio::time::timeout(Duration::from_secs(60), call)
        .await
        .expect("the interrupted call settles")?
    {
        Ok(outcome) => outcome,
        Err(_) => probe(&fixture, &key, "read", "/recover/public", Some(scope)).await?,
    };
    assert_eq!(
        outcome,
        Outcome::Result("first=ok:read:/held/public;second=denied".to_string()),
        "the recorded admission must complete and the new descendant must be denied"
    );
    assert_eq!(
        permission_effects(&fixture),
        vec!["permission/read/held/public"],
        "recovery repeated the leaf effect or admitted the new descendant"
    );
    let oplog = owner_oplog(&fixture).await?;
    assert_eq!(
        entity_starts(&oplog, PERMISSION_PROBE).collect::<Vec<_>>(),
        leaf,
        "recovery must complete the admitted leaf under its original Start"
    );
    assert_eq!(terminal_count(&oplog, leaf[0]), 1);
    assert_eq!(
        rejection_starts(&oplog).len(),
        1,
        "the descendant started after recovery must leave a durable rejection"
    );
    assert_eq!(unterminated_entity_starts(&oplog), Vec::new());
    assert_no_positional_records_after_last_finished(&oplog);
    Ok(())
}

/// Invokes a no-argument owner method under `scope` and returns its string result.
async fn invoke_owner_method_with_scope(
    fixture: &PermissionFixture,
    method: &str,
    scope: &ScopeCard,
) -> anyhow::Result<String> {
    use golem_api_grpc::proto::golem::worker::{
        AgentInvocationMode, InvocationFreshnessDisposition, InvocationStart,
        invocation_session_result,
    };
    let (_, input) = data_value!().into_parts();
    let result = fixture
        .executor
        .invoke_agent_session(InvocationStart {
            agent_id: Some(fixture.owner_id.clone().into()),
            method_name: Some(method.to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            mode: AgentInvocationMode::Await as i32,
            schedule_at: None,
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            component_owner_account_id: Some(fixture.owner_component.account_id.into()),
            environment_id: Some(fixture.owner_component.environment_id.into()),
            auth_ctx: Some(fixture.executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition: InvocationFreshnessDisposition::MayExist as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: Some(scope.try_into().map_err(anyhow::Error::msg)?),
            origin_invocation: None,
            external_tool: None,
        })
        .await?;
    let Some(invocation_session_result::Result::MethodResult(value)) = result.result else {
        anyhow::bail!("unexpected owner method result: {:?}", result.result);
    };
    match SchemaValue::try_from(value).map_err(anyhow::Error::msg)? {
        SchemaValue::String(value) => Ok(value),
        other => anyhow::bail!("unexpected owner method value: {other:?}"),
    }
}

/// Tool and middleware code behaves as if it were inlined in the owner agent, so a scope
/// revocation consumed by a middleware Store must have the same effect on that Store's
/// Underlying calls as on the owner's later ambient calls. The property is that the two
/// outcomes are equal, whichever outcome the scope policy selects.
#[test]
#[timeout("3m")]
async fn scope_revocation_consumed_by_middleware_gives_ambient_and_underlying_the_same_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_runtime_bypass_owner")] owner: &PrecompiledComponent,
    #[tagged_as("tool_runtime_bypass_provider")] provider: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = Arc::new(
        permission_fixture(
            last_unique_id,
            deps,
            owner,
            provider,
            vec![
                owner_grant("read", "/wait-delete/public"),
                owner_grant("read", "/public"),
                owner_grant("delete", "/public"),
            ],
        )
        .await?,
    );
    let root = CardId::new();
    let scope = scope_card(
        root,
        Vec::new(),
        vec![concrete_owner_grant(&fixture, "delete", "/public")],
    );
    let call = tokio::spawn({
        let fixture = fixture.clone();
        let scope = scope.clone();
        async move {
            invoke_owner_method_with_scope(&fixture, "underlying_then_ambient_delete", &scope).await
        }
    });
    // The middleware waits at a promise. The revocation is queued while every Store of the owner
    // is blocked, and the middleware Store consumes it at its Underlying admission.
    let checkpoint = wait_for_effect(&fixture, "wait-ready/").await;
    revoke_card(&fixture, root).await?;
    fixture
        .executor
        .complete_promise(
            &PromiseId {
                agent_id: fixture.owner_id.clone(),
                oplog_idx: OplogIndex::from_u64(checkpoint.parse()?),
            },
            Vec::new(),
        )
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(60), call)
        .await
        .expect("the owner invocation settles")??;
    let (underlying, ambient) = result
        .strip_prefix("underlying=")
        .and_then(|rest| rest.split_once(";ambient="))
        .ok_or_else(|| anyhow::anyhow!("unexpected owner result: {result}"))?;
    assert_eq!(
        underlying, ambient,
        "the middleware Underlying delete and the owner's ambient delete must get the same outcome after the scope revocation"
    );
    let deletes = permission_effects(&fixture)
        .iter()
        .filter(|effect| *effect == "permission/delete/public")
        .count();
    assert_eq!(
        deletes,
        if underlying == "ok:delete:/public" {
            2
        } else {
            0
        },
        "the provider effects must match the outcomes ({result})"
    );
    let oplog = owner_oplog(&fixture).await?;
    assert!(
        oplog.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::CardRevokedCascade(params) if params.revoked_card_ids.contains(&root)
        )),
        "the scope root revocation must be consumed during the invocation"
    );
    assert_eq!(unterminated_entity_starts(&oplog), Vec::new());
    Ok(())
}
