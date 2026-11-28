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
use serde_json::{Value, json};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("permissions_effect")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn permission(pattern: &str) -> PolymorphicPermissionPattern {
    parse_polymorphic_permission(pattern).expect("valid test permission")
}

fn probe(wallet_kinds: &[&str], inspect_allowed: bool) -> Value {
    json!({ "walletKinds": wallet_kinds, "inspectAllowed": inspect_allowed, "scopeExpiresAt": null })
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn effect_sdk_manages_permission_cards_end_to_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("permissions_effect")] permissions_effect: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, permissions_effect)
        .unique()
        .update_agent_provision_config("EffectPermissionCaller", |config| {
            config.initial_permissions.lower_bound.positive.extend([
                permission("card(*) @ * : derive : *"),
                permission("card(*) @ * : inspect : *"),
                permission("card(*) @ * : revoke : *"),
                permission("card(*) @ * : install : *"),
            ]);
        })
        .without_default_host_permissions("EffectPermissionCallee")
        .store()
        .await?;
    for agent_type in ["EffectPermissionCaller", "EffectPermissionCallee"] {
        let card = component
            .metadata
            .agent_type_provision_configs
            .get(&AgentTypeName(agent_type.to_string()))
            .map(|config| &config.initial_permissions)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing initial card for {agent_type}"))?;
        registry.add_card(StoredCard::Polymorphic(card));
    }

    let caller = agent_key!("EffectPermissionCaller", "caller");
    let callee_name = "callee";
    let callee = agent_key!("EffectPermissionCallee", callee_name);
    let caller_worker = executor.start_agent(&component.id, caller.clone()).await?;
    let callee_worker = executor.start_agent(&component.id, callee.clone()).await?;

    let result = executor
        .invoke_and_await_agent(&component, &caller, "run", data_value!(callee_name))
        .await?
        .into_typed::<String>()?;
    let result: Value = serde_json::from_str(&result)?;

    let inspect_grant = json!({
        "class": "card", "owner": "*", "recipient": "*", "verb": "inspect", "resourceId": "*"
    });
    assert_eq!(
        result["derivedView"],
        json!({
            "lowerPositive": [inspect_grant],
            "lowerNegative": [],
            "upperPositive": [inspect_grant],
            "upperNegative": [],
        }),
        "{result:#}"
    );
    assert_eq!(result["childParentMatches"], json!(true), "{result:#}");
    assert_eq!(result["childExpires"], json!(true), "{result:#}");
    assert_eq!(result["isoChildExpires"], json!(true), "{result:#}");
    assert_eq!(result["cardExpires"], json!(false), "{result:#}");
    assert_eq!(result["templateDerive"], json!("ok"), "{result:#}");
    assert_eq!(result["walletKinds"], json!(["Template"]), "{result:#}");

    assert_eq!(
        result["calleeBefore"],
        probe(&["Template"], false),
        "{result:#}"
    );
    assert_eq!(
        result["calleePerCallScope"],
        probe(&["Scope", "Template"], true),
        "{result:#}"
    );
    assert_eq!(
        result["calleeDefaultScope"],
        probe(&["Scope", "Template"], true),
        "{result:#}"
    );
    assert_eq!(
        result["calleeAfterScope"],
        probe(&["Template"], false),
        "{result:#}"
    );
    assert_eq!(result["scopeCardStillUsable"], json!(true), "{result:#}");

    assert_eq!(result["scopeExpiresAt"], Value::Null, "{result:#}");

    let expiring = result["expiringExpiresAt"].clone();
    assert!(expiring.is_string(), "{result:#}");
    let expires_in = result["expiringExpiresInSeconds"]
        .as_f64()
        .expect("expiry in seconds");
    assert!((3500.0..=3700.0).contains(&expires_in), "{result:#}");
    assert_eq!(
        result["walletKindsAfterSelfInstall"],
        json!(["Concrete", "Template"]),
        "{result:#}"
    );
    assert_eq!(result["installedOnSelfExpiresAt"], expiring, "{result:#}");
    assert_eq!(result["expiringScopeExpiresAt"], expiring, "{result:#}");
    assert_eq!(
        result["calleeExpiringScope"],
        json!({
            "walletKinds": ["Scope", "Template"],
            "inspectAllowed": true,
            "scopeExpiresAt": expiring,
        }),
        "{result:#}"
    );

    assert_eq!(
        result["calleeAfterInstall"],
        probe(&["Concrete", "Template"], true),
        "{result:#}"
    );
    assert_eq!(
        result["installedCardMoved"],
        json!("PermissionsHostError"),
        "{result:#}"
    );

    assert_eq!(result["revokedCount"], json!(4), "{result:#}");
    assert_eq!(
        result["revokedCardReuse"],
        json!("PermissionsHostError"),
        "{result:#}"
    );
    assert_eq!(
        result["calleeAfterRevoke"],
        probe(&["Template"], false),
        "{result:#}"
    );

    assert_eq!(
        result["releasedScopeCall"],
        json!("ScopeCardUnavailableError"),
        "{result:#}"
    );
    assert_eq!(
        result["recipientMismatch"],
        json!("install-recipient-mismatch"),
        "{result:#}"
    );
    assert_eq!(
        result["accountHolder"],
        json!("not-permitted"),
        "{result:#}"
    );

    let stale = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "staleScopeCall",
            data_value!(callee_name),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(stale, "RpcCallError:denied");

    assert_eq!(
        result["calleeReceivedTemplate"],
        json!({ "template": true, "concrete": false }),
        "{result:#}"
    );
    assert_eq!(
        result["templateSent"],
        json!("PermissionsHostError"),
        "{result:#}"
    );

    let targets: Vec<_> = worker_service
        .card_transfers()
        .iter()
        .map(|transfer| transfer.target_agent_id)
        .collect();
    assert_eq!(
        targets,
        vec![callee_worker.agent_id, caller_worker.agent_id],
        "the installed card reaches the callee and the self-installed card reaches the caller"
    );

    let denied = executor
        .invoke_and_await_agent(&component, &callee, "deriveWithoutAuthority", data_value!())
        .await?
        .into_typed::<String>()?;
    assert_eq!(denied, "not-permitted");

    executor.check_oplog_is_queryable(&caller_worker).await?;
    executor.check_oplog_is_queryable(&callee_worker).await?;
    Ok(())
}
