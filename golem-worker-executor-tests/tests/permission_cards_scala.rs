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

//! Runs the shared permission-card scenario through the typed Scala SDK API.

use crate::Tracing;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::card::{
    PolymorphicPermissionPattern, StoredCard, parse_polymorphic_permission,
};
use golem_common::schema::SchemaType;
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
inherit_test_dep!(
    #[tagged_as("tool_streaming_scala")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn card_management_permissions() -> Vec<PolymorphicPermissionPattern> {
    ["derive", "inspect", "install", "revoke"]
        .into_iter()
        .map(|verb| {
            parse_polymorphic_permission(&format!("card(*) @ * : {verb} : *"))
                .expect("valid card permission")
        })
        .collect()
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn scala_sdk_runs_the_shared_permission_card_scenario(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] tool_streaming_scala: &PrecompiledComponent,
    _tracing: &Tracing,
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
        .component_dep(&context.default_environment_id, tool_streaming_scala)
        .unique()
        .update_agent_provision_config("ScalaPermissionCardAgent", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(card_management_permissions());
        })
        .update_agent_provision_config("ScalaPermissionCardTarget", |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .retain(|permission| !matches!(permission, PolymorphicPermissionPattern::Card(_)));
        })
        .store()
        .await?;

    // The SDK declares a template card parameter as
    // `permission-card { polymorphic: true }` and a concrete card parameter as
    // `polymorphic: false`.
    let target_type = component
        .metadata
        .agent_types
        .iter()
        .map(|agent_type| &agent_type.declaration)
        .find(|agent_type| agent_type.type_name.0 == "ScalaPermissionCardTarget")
        .ok_or_else(|| anyhow::anyhow!("missing ScalaPermissionCardTarget agent type"))?;
    let card_parameter_polymorphism = |method_name: &str| {
        target_type
            .methods
            .iter()
            .find(|method| method.name == method_name)
            .and_then(|method| method.input_schema.fields().first())
            .and_then(|field| match &field.schema {
                SchemaType::PermissionCard { spec, .. } => Some(spec.polymorphic),
                _ => None,
            })
    };
    assert_eq!(card_parameter_polymorphism("receiveTemplate"), Some(true));
    assert_eq!(card_parameter_polymorphism("receiveConcrete"), Some(false));

    // The registry knows the initial card of every agent type, as in production: the card
    // service treats a card that the registry does not have as revoked.
    for agent_type in ["ScalaPermissionCardAgent", "ScalaPermissionCardTarget"] {
        let card = component
            .metadata
            .agent_type_provision_configs
            .get(&AgentTypeName(agent_type.to_string()))
            .map(|config| &config.initial_permissions)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing initial card for {agent_type}"))?;
        registry.add_card(StoredCard::Polymorphic(card));
    }

    let caller = agent_key!("ScalaPermissionCardAgent", "scenario");
    executor.start_agent(&component.id, caller.clone()).await?;

    let observations = executor
        .invoke_and_await_agent(&component, &caller, "scenario", data_value!("target"))
        .await?
        .into_typed::<Vec<String>>()?;

    assert_eq!(
        observations,
        vec![
            // The initial card of the agent type is a template card.
            "root-kind=template",
            "derived-inspect=true",
            "child-parent=true",
            "child-expiry=true",
            // Deriving from the template card returns concrete children of it,
            // which the wallet lists as concrete cards once installed.
            "template-derive=true",
            "template-children=concrete,concrete,concrete",
            // A scope card expires with its earliest root card; a scope card
            // of a root card without expiry does not expire.
            "scope-expiry=true",
            "root-scope-expiry=None",
            // The target agent type has no card permissions of its own; a scope
            // card grants card:inspect only while the awaited call runs.
            "before=false",
            "per-call=true",
            "default=true",
            "cancelable=true",
            "after=false",
            "scope-usable=true",
            "denied-derive=NotPermitted",
            // The target's wallet holds its initial template card; during a
            // call with a scope card it also lists that card as a scope entry,
            // with the expiry of the caller's scope card.
            "kinds-plain=template",
            "kinds-lent=scope,template",
            "lent-expiry=true",
            "lent-no-expiry=None",
            "installed=true",
            "target-expiring=1",
            "target-inspect=true",
            "self-installed=true",
            "held-before=true",
            "self-held-kind=Some(concrete)",
            // The revoked card and the grandchild derived from it; the holder
            // loses the revoked card.
            "revoked=Some(Right(2))",
            "held-after=false",
            // Revoking the parent of the card installed on the target removes
            // the target's authority.
            // The parent and the card installed on the target; the self-held
            // branch was revoked before.
            "parent-revoked=Right(2)",
            "target-inspect-after-revoke=false",
            "target-expiring-after-revoke=0",
            "moved-child=true",
            "mismatch=Left(InstallRecipientMismatch)",
            "children-revoked=List(Right(1), Right(1), Right(1))",
        ]
    );

    // The scope card of the previous invocation is retired: holding and
    // releasing it is safe, but it carries no authority any more.
    let stale = executor
        .invoke_and_await_agent(&component, &caller, "staleScope", data_value!("target"))
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        stale,
        vec![
            "attach-failed=true",
            "stale-parent=Left(NotPermitted)",
            "released=true"
        ]
    );

    // Sending the template card moves the caller's handle, and the callee
    // decodes it as the declared template card type. An argument carries the
    // card but does not install it: the callee's wallet holds only its own
    // initial template card, and without card:inspect of its own the callee
    // cannot inspect the card.
    let sent = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "sendTemplate",
            data_value!("template-target"),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        sent,
        vec![
            "received-parents=0",
            "received-inspect=false",
            "sent-moved=true",
            "receiver-kinds=template",
        ]
    );
    Ok(())
}
