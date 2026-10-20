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

//! The shared permission-card scenario, driven through the TypeScript SDK's typed
//! `permissions` API in `test-components/agent-sdk-ts/src/permissions.ts`.

use crate::Tracing;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget};
use golem_common::model::agent::{
    AgentInvocationMode, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, ScopeCard, StoredCard, parse_polymorphic_permission,
};
use golem_common::model::component::{ComponentDto, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationOutput, IdempotencyKey, OwnedAgentId, PromiseId,
};
use golem_common::schema::{FromSchema, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{
    CardRevokeResult, CardService, CardState, RuntimeCardCreation,
};
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestCardService, TestContext, TestExecutorOverrides,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

const CALLER: &str = "PermissionCaller";
const CALLEE: &str = "PermissionCallee";
const NO_AUTHORITY: &str = "PermissionNoAuthority";

/// Runtime-created cards and revocations. Other cards are live as in [`TestCardService`].
#[derive(Default)]
struct ScenarioCards {
    cards: Mutex<HashMap<CardId, StoredCard>>,
    revoked: Mutex<HashSet<CardId>>,
}

impl ScenarioCards {
    fn register(&self, card: StoredCard) {
        self.cards.lock().unwrap().insert(card.card_id(), card);
    }

    fn descendants_including_self(&self, card_id: CardId) -> Vec<CardId> {
        let cards = self.cards.lock().unwrap();
        let mut result = vec![card_id];
        let mut index = 0;
        while index < result.len() {
            let parent = result[index];
            for card in cards.values() {
                if card.parent_ids().contains(&parent) && !result.contains(&card.card_id()) {
                    result.push(card.card_id());
                }
            }
            index += 1;
        }
        result
    }
}

struct ScenarioCardService {
    cards: Arc<ScenarioCards>,
}

#[async_trait]
impl CardService for ScenarioCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.cards
            .revoked
            .lock()
            .unwrap()
            .extend(card_ids.iter().copied());
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let mut states = TestCardService.check_cards(card_ids).await?;
        let cards = self.cards.cards.lock().unwrap();
        let revoked = self.cards.revoked.lock().unwrap();
        for (card_id, state) in states.iter_mut() {
            if revoked.contains(card_id) {
                *state = CardState::Revoked;
            } else if let Some(card) = cards.get(card_id) {
                *state = CardState::Live(Box::new(card.clone()));
            }
        }
        Ok(states)
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        self.cards.register(card.clone());
        Ok(RuntimeCardCreation::Created(Box::new(card)))
    }

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        let revoked = self.cards.descendants_including_self(card_id);
        self.cards
            .revoked
            .lock()
            .unwrap()
            .extend(revoked.iter().copied());
        Ok(CardRevokeResult::Revoked(revoked))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        let cards = self.cards.cards.lock().unwrap();
        let revoked = self.cards.revoked.lock().unwrap();
        let mut result = vec![card.card_id()];
        let mut pending = card.parent_ids().to_vec();
        while let Some(parent) = pending.pop() {
            if result.contains(&parent) || revoked.contains(&parent) {
                continue;
            }
            result.push(parent);
            if let Some(parent_card) = cards.get(&parent) {
                pending.extend(parent_card.parent_ids().iter().copied());
            }
        }
        Ok(result)
    }
}

type LocalCardTransferDelivery = dyn Fn(
        AgentId,
        EnvironmentId,
        uuid::Uuid,
        CardId,
        StoredCard,
    ) -> futures::future::BoxFuture<'static, Result<(), WorkerProxyError>>
    + Send
    + Sync;

/// Delivers card transfers to the test executor's own gRPC endpoint. The default test proxy
/// sends them to the public worker API, which does not run in executor tests.
struct LocalCardTransferProxy {
    inner: Arc<dyn WorkerProxy>,
    delivery: Arc<Mutex<Option<Arc<LocalCardTransferDelivery>>>>,
}

#[async_trait]
impl WorkerProxy for LocalCardTransferProxy {
    async fn prepare(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        caller_agent_id: &AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        auth_ctx: &AuthCtx,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .prepare(
                owned_agent_id,
                method_name,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                principal,
                auth_ctx,
            )
            .await
    }

    async fn start(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        caller_agent_id: &AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        auth_ctx: &AuthCtx,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .start(
                owned_agent_id,
                method_name,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                principal,
                auth_ctx,
            )
            .await
    }

    async fn invoke_agent(
        &self,
        agent_id: &AgentId,
        method_name: String,
        method_parameters: SchemaValue,
        mode: AgentInvocationMode,
        schedule_at: Option<DateTime<Utc>>,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        caller_agent_id: AgentId,
        caller_env: HashMap<String, String>,
        caller_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<AgentInvocationOutput, WorkerProxyError> {
        self.inner
            .invoke_agent(
                agent_id,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                idempotency_key,
                freshness_disposition,
                caller_agent_id,
                caller_env,
                caller_stack,
                config,
                principal,
                environment_id,
                auth_ctx,
                scope_card,
            )
            .await
    }

    async fn deliver_card_transfer(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        transfer_id: uuid::Uuid,
        source_card_id: CardId,
        card: &StoredCard,
    ) -> Result<(), WorkerProxyError> {
        let delivery = self.delivery.lock().unwrap().clone().ok_or_else(|| {
            WorkerProxyError::InternalError(WorkerExecutorError::runtime(
                "local card-transfer delivery is not connected",
            ))
        })?;
        delivery(
            target_agent_id.clone(),
            environment_id,
            transfer_id,
            source_card_id,
            card.clone(),
        )
        .await
    }

    async fn update(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_revision: ComponentRevision,
        mode: golem_api_grpc::proto::golem::worker::UpdateMode,
        disable_wakeup: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .update(
                owned_agent_id,
                target_revision,
                mode,
                disable_wakeup,
                auth_ctx,
            )
            .await
    }

    async fn resume(
        &self,
        agent_id: &AgentId,
        force: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner.resume(agent_id, force, auth_ctx).await
    }

    async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cutoff: &OplogIndex,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .fork_worker(
                source_agent_id,
                target_agent_id,
                oplog_index_cutoff,
                auth_ctx,
            )
            .await
    }

    async fn revert(
        &self,
        agent_id: &AgentId,
        target: RevertWorkerTarget,
        resolved_revert: Option<ResolvedRevert>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .revert(agent_id, target, resolved_revert, auth_ctx)
            .await
    }

    async fn complete_promise(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .complete_promise(promise_id, data, auth_ctx)
            .await
    }

    async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: IdempotencyKey,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .cancel_invocation(agent_id, idempotency_key, auth_ctx)
            .await
    }

    async fn lookup_invocation_status(
        &self,
        agent_id: &AgentId,
        idempotency_key: IdempotencyKey,
        environment_id: Option<EnvironmentId>,
        auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::InvocationStatus, WorkerProxyError> {
        self.inner
            .lookup_invocation_status(agent_id, idempotency_key, environment_id, auth_ctx)
            .await
    }

    async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_revision: ComponentRevision,
        idempotency_key: IdempotencyKey,
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .process_oplog_entries(
                target_agent_id,
                environment_id,
                component_revision,
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
                auth_ctx,
            )
            .await
    }
}

fn connect_local_card_transfer_delivery(
    delivery: &Arc<Mutex<Option<Arc<LocalCardTransferDelivery>>>>,
    executor: &TestWorkerExecutor,
) {
    let client = executor.client.clone();
    *delivery.lock().unwrap() = Some(Arc::new(
        move |target_agent_id, environment_id, transfer_id, source_card_id, card| {
            let mut client = client.clone();
            Box::pin(async move {
                use golem_api_grpc::proto::golem::workerexecutor::v1::{
                    DeliverCardTransferRequest, deliver_card_transfer_response,
                };

                let response = client
                    .deliver_card_transfer(DeliverCardTransferRequest {
                        target_agent_id: Some(target_agent_id.into()),
                        environment_id: Some(environment_id.into()),
                        transfer_id: Some(transfer_id.into()),
                        card: desert_rust::serialize_to_byte_vec(&card).map_err(|error| {
                            WorkerProxyError::InternalError(WorkerExecutorError::unknown(format!(
                                "failed to serialize local card transfer: {error}"
                            )))
                        })?,
                        auth_ctx: Some(AuthCtx::System.into()),
                        source_card_id: Some(source_card_id.0.into()),
                    })
                    .await?
                    .into_inner();
                match response.result {
                    Some(deliver_card_transfer_response::Result::Success(_)) => Ok(()),
                    Some(deliver_card_transfer_response::Result::Failure(error)) => {
                        Err(WorkerProxyError::InternalError(
                            WorkerExecutorError::try_from(error).map_err(|error| {
                                WorkerProxyError::InternalError(WorkerExecutorError::unknown(error))
                            })?,
                        ))
                    }
                    None => Err(WorkerProxyError::InternalError(
                        WorkerExecutorError::unknown(
                            "local permission-card delivery returned no result",
                        ),
                    )),
                }
            })
        },
    ));
}

struct Scenario {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    caller: ParsedAgentId,
}

async fn start_scenario(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    agent_sdk_ts: &PrecompiledComponent,
    caller_name: &str,
) -> anyhow::Result<Scenario> {
    let cards = Arc::new(ScenarioCards::default());
    let delivery = Arc::new(Mutex::new(None));
    let overrides = TestExecutorOverrides {
        create_card_service: Some(Arc::new({
            let cards = cards.clone();
            move || {
                Arc::new(ScenarioCardService {
                    cards: cards.clone(),
                })
            }
        })),
        wrap_worker_proxy: Some(Arc::new({
            let delivery = delivery.clone();
            move |inner| {
                Arc::new(LocalCardTransferProxy {
                    inner,
                    delivery: delivery.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, context, overrides).await?;
    connect_local_card_transfer_delivery(&delivery, &executor);

    let caller_grants = [
        "card(*) @ * : inspect : *",
        "card(*) @ * : derive : *",
        "card(*) @ * : revoke : *",
        "card(*) @ * : install : *",
        "env(*/*/*/*/*) @ * : read : SCOPE_ENV",
    ]
    .map(|grant| parse_polymorphic_permission(grant).expect("valid scenario grant"));
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .update_agent_provision_config(CALLER, |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(caller_grants);
        })
        .without_default_host_permissions(CALLEE)
        .with_env(
            CALLEE,
            vec![("SCOPE_ENV".to_string(), "visible".to_string())],
        )
        .without_default_host_permissions(NO_AUTHORITY)
        .store()
        .await?;

    let caller = agent_id!(CALLER, caller_name);
    let caller_card = component
        .metadata
        .agent_type_initial_permission_card(&caller.agent_type)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing {CALLER} initial permission card"))?;
    cards.register(StoredCard::Polymorphic(caller_card));
    executor.start_agent(&component.id, caller.clone()).await?;

    Ok(Scenario {
        executor,
        component,
        caller,
    })
}

#[derive(Debug, FromSchema, PartialEq)]
struct WalletDerivation {
    parent_from_wallet: bool,
    parent_grants: Vec<String>,
    parent_expires: bool,
    child_of_parent: bool,
    child_grants: Vec<String>,
    child_lifespan_seconds: f64,
}

#[derive(Debug, FromSchema, PartialEq)]
struct ScopeRpc {
    before: String,
    during: String,
    after: String,
    client_default: String,
    suppressed: String,
    scope_card_kept: bool,
}

#[derive(Debug, FromSchema, PartialEq)]
struct InstallAndRevoke {
    wallet_before: Vec<String>,
    installed: bool,
    moved_error: String,
    wallet_after_install: Vec<String>,
    env_after_install: String,
    revoked_count: f64,
    wallet_after_revoke: Vec<String>,
    env_after_revoke: String,
}

#[derive(Debug, FromSchema, PartialEq)]
struct WalletCardKinds {
    own_before: Vec<String>,
    own: Vec<String>,
    template_child_is_concrete: bool,
    long_expires_at: f64,
    parent_expires_at: f64,
    scope_expires_at: f64,
    callee_without_scope: Vec<String>,
    callee_with_scope: Vec<String>,
    callee_scope_expires_at: f64,
}

#[derive(Debug, FromSchema, PartialEq)]
struct CardRoundTrip {
    moved_after_send: bool,
    same_card: bool,
    returned_inspectable: bool,
    template_moved_after_send: bool,
    same_template: bool,
    returned_template_inspectable: bool,
}

fn kinds(kinds: &[&str]) -> Vec<String> {
    kinds.iter().map(|kind| kind.to_string()).collect()
}

#[derive(Debug, FromSchema, PartialEq)]
struct RecipientMismatch {
    error: String,
    moved: bool,
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_permissions_derive_and_inspect_wallet_cards(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let scenario = start_scenario(deps, &context, agent_sdk_ts, "derivation").await?;

    let derivation = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "walletDerivation",
            data_value!(),
        )
        .await?
        .into_typed::<WalletDerivation>()?;

    assert_eq!(
        derivation,
        WalletDerivation {
            parent_from_wallet: true,
            parent_grants: vec![
                "card(*) @ * : inspect : *".to_string(),
                "env(*/*/*/*/*) @ * : read : SCOPE_ENV".to_string(),
            ],
            parent_expires: false,
            child_of_parent: true,
            child_grants: vec!["env(*/*/*/*/*) @ * : read : SCOPE_ENV".to_string()],
            child_lifespan_seconds: derivation.child_lifespan_seconds,
        }
    );
    assert!(
        (3590.0..=3600.0).contains(&derivation.child_lifespan_seconds),
        "a one-hour lifespan must expire in about an hour, got {}s",
        derivation.child_lifespan_seconds
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_permissions_scope_card_extends_callee_authority_for_one_await_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let scenario = start_scenario(deps, &context, agent_sdk_ts, "scope").await?;

    let observed = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "scopeRpc",
            data_value!("scope-callee"),
        )
        .await?
        .into_typed::<ScopeRpc>()?;

    assert_eq!(
        observed,
        ScopeRpc {
            before: "<absent>".to_string(),
            during: "visible".to_string(),
            after: "<absent>".to_string(),
            client_default: "visible".to_string(),
            suppressed: "<absent>".to_string(),
            scope_card_kept: true,
        }
    );

    // A scope card is valid only in the invocation that derived it.
    scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "keepScopeCard",
            data_value!(),
        )
        .await?;
    let stale = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "useKeptScopeCard",
            data_value!("scope-callee"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(stale, "denied");
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_permissions_install_and_revoke_move_authority_between_agents(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let scenario = start_scenario(deps, &context, agent_sdk_ts, "install").await?;

    let observed = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "installAndRevoke",
            data_value!("install-callee"),
        )
        .await?
        .into_typed::<InstallAndRevoke>()?;

    assert_eq!(
        observed,
        InstallAndRevoke {
            // The callee's own initial card is a template; the installed child is concrete.
            wallet_before: kinds(&["template"]),
            installed: true,
            moved_error: observed.moved_error.clone(),
            wallet_after_install: kinds(&["concrete", "template"]),
            env_after_install: "visible".to_string(),
            // The revoked parent, the installed child and the child the caller kept.
            revoked_count: 3.0,
            wallet_after_revoke: kinds(&["template"]),
            env_after_revoke: "<absent>".to_string(),
        }
    );
    assert!(
        observed.moved_error.contains("already been moved"),
        "using an installed card must fail loudly, got: {}",
        observed.moved_error
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_permissions_reject_mismatched_recipients_and_missing_authority(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let scenario = start_scenario(deps, &context, agent_sdk_ts, "negative").await?;

    let mismatch = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "recipientMismatch",
            data_value!("mismatch-callee"),
        )
        .await?
        .into_typed::<RecipientMismatch>()?;
    assert_eq!(
        mismatch,
        RecipientMismatch {
            error: "install-recipient-mismatch".to_string(),
            moved: true,
        }
    );

    let no_authority = agent_id!(NO_AUTHORITY, "no-authority");
    scenario
        .executor
        .start_agent(&scenario.component.id, no_authority.clone())
        .await?;
    let error = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &no_authority,
            "deriveFromWallet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(error, "not-permitted");
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_permissions_cards_move_through_rpc_arguments_and_results(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let scenario = start_scenario(deps, &context, agent_sdk_ts, "round-trip").await?;

    let observed = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "cardRoundTrip",
            data_value!("round-trip-callee"),
        )
        .await?
        .into_typed::<CardRoundTrip>()?;

    assert_eq!(
        observed,
        CardRoundTrip {
            moved_after_send: true,
            same_card: true,
            returned_inspectable: true,
            template_moved_after_send: true,
            same_template: true,
            returned_template_inspectable: true,
        }
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_permissions_wallet_labels_card_kinds_and_scope_expiry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let scenario = start_scenario(deps, &context, agent_sdk_ts, "kinds").await?;

    let started = chrono::Utc::now().timestamp() as f64;
    let observed = scenario
        .executor
        .invoke_and_await_agent(
            &scenario.component,
            &scenario.caller,
            "walletCardKinds",
            data_value!("kinds-callee"),
        )
        .await?
        .into_typed::<WalletCardKinds>()?;

    assert_eq!(
        observed,
        WalletCardKinds {
            own_before: kinds(&["template"]),
            // The expiring children of the template, installed on the caller itself, are concrete.
            own: kinds(&["concrete", "concrete", "template"]),
            template_child_is_concrete: true,
            long_expires_at: observed.long_expires_at,
            parent_expires_at: observed.parent_expires_at,
            // The roots expire in two hours, one hour and never; the scope card takes the
            // earliest, although the two-hour root is its first parent.
            scope_expires_at: observed.parent_expires_at,
            callee_without_scope: kinds(&["template"]),
            // The scope card grants `card:inspect`, so the callee sees it in its wallet.
            callee_with_scope: kinds(&["scope", "template"]),
            callee_scope_expires_at: observed.parent_expires_at,
        }
    );
    let lifespan = observed.parent_expires_at - started;
    assert!(
        (3500.0..=3700.0).contains(&lifespan),
        "the one-hour card must expire one hour after it was derived, got {lifespan}s"
    );
    let long_lifespan = observed.long_expires_at - started;
    assert!(
        (7100.0..=7300.0).contains(&long_lifespan),
        "the two-hour card must expire two hours after it was derived, got {long_lifespan}s"
    );
    Ok(())
}
