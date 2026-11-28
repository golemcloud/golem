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
use golem_common::model::agent::AgentTypeName;
use golem_common::model::card::{
    PolymorphicPermissionPattern, StoredCard, parse_polymorphic_permission,
};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    EmulatedRegistryService, EmulatedWorkerService, LastUniqueId, PrecompiledComponent,
    TestContext, TestExecutorOverrides, WorkerExecutorTestDependencies, WorkerServiceOptions,
    start_with_overrides,
};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

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
    let registry = EmulatedRegistryService::start().await?;
    let worker_service = EmulatedWorkerService::start(WorkerServiceOptions::default()).await?;
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides::default()
            .with_registry(&registry)
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
        registry.add_card(StoredCard::Polymorphic(card));
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
