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

//! Permission-card management through the Go SDK: a `CardIssuer` derives cards
//! from its wallet, inspects them, lends a scope card to awaited calls,
//! installs a card on a `CardReceiver` and revokes it, and the receiver reports
//! the authority it holds, and the kinds of its wallet cards, after each step.

use crate::Tracing;
use async_trait::async_trait;

use golem_common::model::agent::AgentTypeName;
use golem_common::model::card::{
    CardId, CardManagedByRuntimeDerived, PolymorphicPermissionPattern, StoredCard,
    parse_polymorphic_permission,
};
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{
    CardRevokeResult, CardService, CardState, RuntimeCardCreation,
};
use golem_worker_executor_test_utils::{
    EmulatedWorkerService, LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, WorkerServiceOptions, start_with_overrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// The cards the registry knows, and the ones it revoked.
#[derive(Default)]
struct CardRegistry {
    cards: RwLock<HashMap<CardId, StoredCard>>,
    revoked: RwLock<HashSet<CardId>>,
}

impl CardRegistry {
    fn add(&self, card: StoredCard) {
        self.cards.write().unwrap().insert(card.card_id(), card);
    }

    fn ancestors_including_self(&self, card_id: CardId) -> Vec<CardId> {
        let cards = self.cards.read().unwrap();
        let mut out = Vec::new();
        let mut pending = vec![card_id];
        while let Some(id) = pending.pop() {
            if out.contains(&id) {
                continue;
            }
            out.push(id);
            if let Some(card) = cards.get(&id) {
                pending.extend(card.parent_ids().iter().copied());
            }
        }
        out
    }
}

/// A card service over [`CardRegistry`] whose revocation cascades to every
/// card derived from the revoked one, as the registry service's does.
struct RegistryCardService(Arc<CardRegistry>);

#[async_trait]
impl CardService for RegistryCardService {
    async fn record_revoked_cards(&self, card_ids: &[CardId]) {
        self.0
            .revoked
            .write()
            .unwrap()
            .extend(card_ids.iter().copied());
    }

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let cards = self.0.cards.read().unwrap();
        let revoked = self.0.revoked.read().unwrap();
        Ok(card_ids
            .into_iter()
            .map(|card_id| {
                let state = if revoked.contains(&card_id) {
                    CardState::Revoked
                } else if let Some(card) = cards.get(&card_id) {
                    CardState::Live(Box::new(card.clone()))
                } else {
                    CardState::Unknown
                };
                (card_id, state)
            })
            .collect())
    }

    async fn create_runtime_card(
        &self,
        card: StoredCard,
        _provenance: CardManagedByRuntimeDerived,
    ) -> Result<RuntimeCardCreation, WorkerExecutorError> {
        self.0.add(card.clone());
        Ok(RuntimeCardCreation::Created(Box::new(card)))
    }

    async fn revoke_card(&self, card_id: CardId) -> Result<CardRevokeResult, WorkerExecutorError> {
        if self.0.revoked.read().unwrap().contains(&card_id) {
            return Ok(CardRevokeResult::AlreadyRevoked(format!(
                "card {card_id} is already revoked"
            )));
        }
        let known = self
            .0
            .cards
            .read()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let cascade = known
            .into_iter()
            .filter(|id| self.0.ancestors_including_self(*id).contains(&card_id))
            .collect::<Vec<_>>();
        self.0
            .revoked
            .write()
            .unwrap()
            .extend(cascade.iter().copied());
        Ok(CardRevokeResult::Revoked(cascade))
    }

    async fn live_ancestor_ids_including_self(
        &self,
        card: &StoredCard,
    ) -> Result<Vec<CardId>, WorkerExecutorError> {
        let revoked = self.0.revoked.read().unwrap().clone();
        Ok(self
            .0
            .ancestors_including_self(card.card_id())
            .into_iter()
            .filter(|id| !revoked.contains(id))
            .collect())
    }
}

fn card_grant(verb: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(&format!("card(*) @ * : {verb} : *"))
        .expect("valid card permission")
}

/// The shared permission-card scenario. The issuer holds the four card verbs;
/// the receiver holds none, so every card authority it reports comes from a
/// card the issuer lent or installed.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_permission_cards_derive_lend_install_and_revoke(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let registry = Arc::new(CardRegistry::default());
    let worker_service = EmulatedWorkerService::start(WorkerServiceOptions::default()).await?;
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new({
                let registry = registry.clone();
                move || Arc::new(RegistryCardService(registry.clone()))
            })),
            ..Default::default()
        }
        .with_worker_service(&worker_service),
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .unique()
        .update_agent_provision_config("CardIssuer", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(["derive", "inspect", "install", "revoke"].map(card_grant));
        })
        .update_agent_provision_config("CardReceiver", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|grant| !matches!(grant, PolymorphicPermissionPattern::Card(_)));
        })
        .store()
        .await?;
    for agent_type in ["CardIssuer", "CardReceiver"] {
        let card = component
            .metadata
            .agent_type_provision_configs
            .get(&AgentTypeName(agent_type.to_string()))
            .map(|config| &config.initial_permissions)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{agent_type} has no initial card"))?;
        registry.add(StoredCard::Polymorphic(card));
    }

    let issuer = agent_key!("CardIssuer", "go-card-issuer");
    executor.start_agent(&component.id, issuer.clone()).await?;
    let report = executor
        .invoke_and_await_agent(
            &component,
            &issuer,
            "scenario",
            data_value!("go-card-receiver"),
        )
        .await?
        .into_typed::<Vec<String>>()?;

    assert_eq!(
        report,
        [
            "wallet-grants-inspect=true",
            "wallet-kinds=template",
            "from-wallet-parents=1",
            "from-wallet-grants=1/0/0/0",
            "from-wallet-grant-is-requested=true",
            "from-wallet-expires=false",
            "child-parent-is-from-wallet=true",
            "child-expires-within-the-hour=true",
            "receiver-before=not-permitted",
            "scope-parent-is-source=true",
            "receiver-with-scope=ok",
            "receiver-wallet-with-scope=scope@none,template",
            "receiver-with-default-scope=ok",
            "receiver-call-scope-over-default=not-permitted",
            "receiver-call-scope-over-empty-default=ok",
            "reflected-default-scope=ok",
            "reflected-call-scope-over-default=not-permitted",
            "dynamic-default-scope=ok",
            "dynamic-call-scope-over-default=ok",
            "receiver-trigger-and-schedule=not-permitted,not-permitted",
            "receiver-after-scope=not-permitted",
            "scope-card-kept=true",
            "install=ok",
            "child-moved-by-install=true",
            "receiver-after-install=ok",
            "receiver-wallet-after-install=concrete,template",
            "revoke=ok",
            "revoked-count=3",
            "from-wallet-moved-by-revoke=true",
            "receiver-after-revoke=not-permitted",
            "receiver-derive-from-wallet=not-permitted",
            "install-elsewhere=install-recipient-mismatch",
            "unknown-class=unknown-resource-class",
        ]
    );

    // A scope card is valid only in the invocation that derived it.
    let stale = executor
        .invoke_and_await_agent(&component, &issuer, "use-stale-scope", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        stale,
        [
            "attach=remote-denied",
            "derive-from=not-permitted",
            "released=true"
        ]
    );
    Ok(())
}
