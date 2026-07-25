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
use golem_client::api::{RegistryServiceClient, WorkerClient};
use golem_client::model::StoredCard;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::card::parse_polymorphic_permission;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::component::ComponentName;
use golem_common::model::{AgentStatus, IdempotencyKey, PromiseId};
use golem_common::{agent_id, data_value};
use golem_schema::model::{CardId as SchemaCardId, ComponentId as SchemaComponentId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

fn card_id(card: &StoredCard) -> Uuid {
    match card {
        StoredCard::Concrete(card) => card.card_id,
        StoredCard::Polymorphic(card) => card.card_id,
    }
}

fn parent_ids(card: &StoredCard) -> &[Uuid] {
    match card {
        StoredCard::Concrete(card) => &card.parent_ids,
        StoredCard::Polymorphic(card) => &card.parent_ids,
    }
}

#[test]
#[timeout("8m")]
#[tracing::instrument]
async fn permission_cards_work_across_services_and_replay(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (app, env) = user.app_and_env().await?;
    let component_name = "permissions-e2e";
    let install_target = RecipientPattern::Agent {
        account: user.account_email.clone(),
        application: app.name.clone(),
        environment: env.name.clone(),
        component: ComponentName(component_name.to_string()),
        agent_type: AgentTypeName("ScopeCardAgent".to_string()),
    };
    let account = user.account_email.as_str();
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name(component_name)
        .try_update_agent_provision_config("ScopeCardAgent", |config| {
            for grant in [
                format!("card({account}) @ * : derive : *"),
                format!("card({account}) @ * : inspect : *"),
                format!("card({account}) @ * : revoke : *"),
                format!(
                    "card({account}) @ * : install : {}",
                    install_target.render()
                ),
            ] {
                config
                    .initial_permissions
                    .lower_bound
                    .positive
                    .push(parse_polymorphic_permission(&grant)?);
            }
            Ok::<_, golem_common::model::card::CardParseError>(())
        })?
        .store()
        .await?;
    let caller = agent_id!("ScopeCardAgent", "permissions-caller");
    let target = agent_id!("ScopeCardAgent", "permissions-target");
    let caller_worker = user.start_agent(&component.id, caller.clone()).await?;
    user.start_agent(&component.id, target.clone()).await?;

    let initial_card_id = component
        .metadata
        .agent_type_initial_permission_card(&AgentTypeName("ScopeCardAgent".to_string()))
        .expect("ScopeCardAgent initial card is missing")
        .card_id
        .0;
    let worker_client = user
        .deps
        .worker_service()
        .worker_http_client(&user.token)
        .await;
    let caller_wallet = worker_client
        .get_agent_wallet(&component.id.0, &caller.to_string())
        .await?;
    assert!(
        caller_wallet
            .iter()
            .any(|card| card_id(card) == initial_card_id),
        "the deployed initial card must be installed before guest code runs"
    );

    let registry_client = user.registry_service_client().await;
    let stored_initial = registry_client.get_card(&initial_card_id).await?;
    assert_eq!(card_id(&stored_initial), initial_card_id);

    let (scope_present, scope_parent_matches, scope_grants_match, scope_card_id) = user
        .invoke_and_await_agent(
            &component,
            &caller,
            "invoke_and_await_scope",
            data_value!("permissions-target"),
        )
        .await?
        .into_typed::<(bool, bool, bool, SchemaCardId)>()?;
    assert_eq!(
        (scope_present, scope_parent_matches, scope_grants_match),
        (true, true, true)
    );
    assert!(
        !user
            .invoke_and_await_agent(&component, &target, "has_card", data_value!(scope_card_id),)
            .await?
            .into_typed::<bool>()?,
        "the exact scope card must not survive invocation end"
    );

    let component_id = SchemaComponentId::from(component.id.0);
    let (parent_id, child_id) = user
        .invoke_and_await_agent(
            &component,
            &caller,
            "derive_and_install_chain",
            data_value!(component_id, caller.to_string(), target.to_string()),
        )
        .await?
        .into_typed::<(SchemaCardId, SchemaCardId)>()?;
    let stored_parent = registry_client.get_card(&parent_id.uuid).await?;
    let stored_child = registry_client.get_card(&child_id.uuid).await?;
    assert_eq!(parent_ids(&stored_parent), &[initial_card_id]);
    assert_eq!(parent_ids(&stored_child), &[parent_id.uuid]);
    assert!(
        user.invoke_and_await_agent(&component, &target, "has_card", data_value!(child_id),)
            .await?
            .into_typed::<bool>()?,
        "the transferred child must be installed in the target wallet"
    );

    let release = user
        .invoke_and_await_agent(&component, &caller, "create_release_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let replay_key = IdempotencyKey::fresh();
    let replay_params = data_value!(component_id, target.to_string(), release.clone());
    user.invoke_agent_with_key(
        &component,
        &caller,
        &replay_key,
        "derive_and_install_after_promise",
        replay_params.clone(),
    )
    .await?;
    user.wait_for_status(
        &caller_worker,
        AgentStatus::Suspended,
        Duration::from_secs(30),
    )
    .await?;
    user.simulated_crash(&caller_worker).await?;
    user.complete_promise(&release, vec![1]).await?;
    let replay_card_id = user
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &replay_key,
            "derive_and_install_after_promise",
            replay_params,
        )
        .await?
        .into_typed::<SchemaCardId>()?;
    assert_eq!(
        card_id(&registry_client.get_card(&replay_card_id.uuid).await?),
        replay_card_id.uuid
    );
    assert!(
        user.invoke_and_await_agent(&component, &target, "has_card", data_value!(replay_card_id),)
            .await?
            .into_typed::<bool>()?,
        "the replayed transfer must install exactly the derived card"
    );

    let revoked = user
        .invoke_and_await_agent(
            &component,
            &caller,
            "revoke_card_by_id",
            data_value!(parent_id),
        )
        .await?
        .into_typed::<u32>()?;
    assert_eq!(revoked, 2, "revocation must cut the parent and child");
    assert!(
        !user
            .invoke_and_await_agent(&component, &target, "has_card", data_value!(child_id),)
            .await?
            .into_typed::<bool>()?,
        "the revoked child must be removed at the target's next boundary"
    );

    let caller_wallet = worker_client
        .get_agent_wallet(&component.id.0, &caller.to_string())
        .await?;
    let target_wallet = worker_client
        .get_agent_wallet(&component.id.0, &target.to_string())
        .await?;
    assert!(
        !caller_wallet
            .iter()
            .any(|card| card_id(card) == parent_id.uuid)
    );
    assert!(
        !target_wallet
            .iter()
            .any(|card| card_id(card) == child_id.uuid)
    );
    assert!(
        target_wallet
            .iter()
            .any(|card| card_id(card) == replay_card_id.uuid),
        "the unrelated replayed transfer must remain installed"
    );

    Ok(())
}
