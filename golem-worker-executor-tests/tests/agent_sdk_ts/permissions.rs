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

use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, StoredCard, parse_polymorphic_permission,
};
use golem_common::model::component::ComponentDto;
use golem_common::schema::FromSchema;
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{
    CardRevokeResult, CardService, CardState, RuntimeCardCreation,
};
use golem_worker_executor_test_utils::{
    EmulatedWorkerService, LastUniqueId, PrecompiledComponent, TestCardService, TestContext,
    TestExecutorOverrides, TestWorkerExecutor, WorkerExecutorTestDependencies,
    WorkerServiceOptions, start_with_overrides,
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

struct Scenario {
    executor: TestWorkerExecutor,
    component: ComponentDto,
    caller: ParsedAgentKey,
}

async fn start_scenario(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    agent_sdk_ts: &PrecompiledComponent,
    caller_name: &str,
) -> anyhow::Result<Scenario> {
    let cards = Arc::new(ScenarioCards::default());
    let worker_service = EmulatedWorkerService::start(WorkerServiceOptions::default()).await?;
    let overrides = TestExecutorOverrides {
        create_card_service: Some(Arc::new({
            let cards = cards.clone();
            move || {
                Arc::new(ScenarioCardService {
                    cards: cards.clone(),
                })
            }
        })),
        ..Default::default()
    }
    .with_worker_service(&worker_service);
    let executor = start_with_overrides(deps, context, overrides).await?;

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

    let caller = agent_key!(CALLER, caller_name);
    let caller_card = component
        .metadata
        .agent_type_provision_configs
        .get(&caller.agent_type)
        .map(|config| &config.initial_permissions)
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

    let no_authority = agent_key!(NO_AUTHORITY, "no-authority");
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

    let started = jiff::Timestamp::now().as_second() as f64;
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
