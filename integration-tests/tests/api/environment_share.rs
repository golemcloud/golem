// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_client::api::{
    AgentClient, AgentError, RegistryServiceClient, RegistryServiceGetEnvironmentShareError,
};
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::environment_share::{EnvironmentShareCreation, EnvironmentShareUpdate};
use golem_common::model::IdempotencyKey;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn share_environment_with_other_user(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;

    let share_creation = EnvironmentShareCreation {
        grantee_account_id: user_2.account_id,
        roles: BTreeSet::from([EnvironmentRole::Admin]),
    };

    let share = client_1
        .create_environment_share(&env.id.0, &share_creation)
        .await?;

    assert_eq!(share.grantee_account_id, user_2.account_id);
    assert_eq!(share.roles, share_creation.roles);

    {
        let fetched_share = client_1.get_environment_share(&share.id.0).await?;
        assert_eq!(fetched_share, share);
    }

    {
        let all_environment_shares = client_1
            .get_environment_environment_shares(&env.id.0)
            .await?;
        assert!(all_environment_shares.values.contains(&share));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_environment_shares(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;

    let share_creation = EnvironmentShareCreation {
        grantee_account_id: user_2.account_id,
        roles: BTreeSet::from([EnvironmentRole::Admin]),
    };

    let share = client_1
        .create_environment_share(&env.id.0, &share_creation)
        .await?;

    client_1
        .delete_environment_share(&share.id.0, share.revision.into())
        .await?;

    {
        let result = client_1.get_environment_share(&share.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentShareError::Error404(_)
            ))
        ));
    }

    {
        let all_environment_shares = client_1
            .get_environment_environment_shares(&env.id.0)
            .await?;
        assert_eq!(all_environment_shares.values, Vec::new());
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_environment_shares(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;

    let share_creation = EnvironmentShareCreation {
        grantee_account_id: user_2.account_id,
        roles: BTreeSet::from([EnvironmentRole::Admin]),
    };

    let share = client_1
        .create_environment_share(&env.id.0, &share_creation)
        .await?;

    let share_update = EnvironmentShareUpdate {
        current_revision: share.revision,
        roles: BTreeSet::from([EnvironmentRole::Viewer]),
    };

    let updated_share = client_1
        .update_environment_share(&share.id.0, &share_update)
        .await?;

    assert_eq!(updated_share.roles, share_update.roles);

    {
        let fetched_share = client_1.get_environment_share(&share.id.0).await?;
        assert_eq!(fetched_share, updated_share);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn invoke_agent_in_shared_environment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let grantee = deps.user().await?;

    // Owner creates app/env and stores a counter component (auto-deploys)
    let (_, env) = owner.app_and_env().await?;
    let component = owner
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;

    // Resolve app and env names from owner's perspective
    let owner_registry_client = deps.registry_service().client(&owner.token).await;
    let app = owner_registry_client
        .get_application(&component.application_id.0)
        .await?;
    let env = owner_registry_client
        .get_environment(&component.environment_id.0)
        .await?;

    // Owner shares the environment with grantee (Admin role — needed for worker creation/invocation)
    owner
        .share_environment(&env.id, &grantee.account_id, &[EnvironmentRole::Admin])
        .await?;

    // Grantee invokes the agent using owner's email
    let agent_id = agent_id!("rpc-counter", "shared-counter-1");

    let grantee_agent_client = deps
        .worker_service()
        .agent_http_client(&grantee.token)
        .await;

    // First call: increment by 5 (with idempotency key to prevent double-increment on retry)
    let inc_key = IdempotencyKey::fresh();
    grantee_agent_client
        .invoke_agent(
            Some(&inc_key.value),
            &golem_client::model::AgentInvocationRequest {
                app_name: app.name.0.clone(),
                env_name: env.name.0.clone(),
                agent_type_name: agent_id.agent_type.to_wit_naming().0.clone(),
                parameters: agent_id.parameters.clone().into(),
                phantom_id: agent_id.phantom_id,
                method_name: "inc_by".to_string(),
                method_parameters: data_value!(5u64).into(),
                mode: golem_client::model::AgentInvocationMode::Await,
                schedule_at: None,
                idempotency_key: None,
                deployment_revision: None,
                owner_account_email: Some(owner.account_email.0.clone()),
            },
        )
        .await?;

    // Second call: get the value
    let get_key = IdempotencyKey::fresh();
    let result = grantee_agent_client
        .invoke_agent(
            Some(&get_key.value),
            &golem_client::model::AgentInvocationRequest {
                app_name: app.name.0.clone(),
                env_name: env.name.0.clone(),
                agent_type_name: agent_id.agent_type.to_wit_naming().0.clone(),
                parameters: agent_id.parameters.clone().into(),
                phantom_id: agent_id.phantom_id,
                method_name: "get_value".to_string(),
                method_parameters: data_value!().into(),
                mode: golem_client::model::AgentInvocationMode::Await,
                schedule_at: None,
                idempotency_key: None,
                deployment_revision: None,
                owner_account_email: Some(owner.account_email.0.clone()),
            },
        )
        .await?;

    // The result should contain the value 5
    let json_result = result
        .result
        .expect("Expected a return value from get_value");
    match json_result {
        golem_common::model::agent::UntypedJsonDataValue::Tuple(tuple) => {
            assert_eq!(tuple.elements.len(), 1);
            match &tuple.elements[0] {
                golem_common::model::agent::UntypedJsonElementValue::ComponentModel(cm) => {
                    assert_eq!(cm.value, serde_json::json!(5));
                }
                other => panic!("Expected ComponentModel element, got {:?}", other),
            }
        }
        other => panic!("Expected Tuple result, got {:?}", other),
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn invoke_agent_in_shared_environment_fails_without_share(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let non_shared_user = deps.user().await?;

    // Owner creates app/env and stores a counter component (auto-deploys)
    let (_, env) = owner.app_and_env().await?;
    let component = owner
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;

    // Resolve app and env names from owner's perspective
    let owner_registry_client = deps.registry_service().client(&owner.token).await;
    let app = owner_registry_client
        .get_application(&component.application_id.0)
        .await?;
    let env = owner_registry_client
        .get_environment(&component.environment_id.0)
        .await?;

    // No share is created — non_shared_user should NOT be able to invoke

    let agent_id = agent_id!("rpc-counter", "no-share-counter");

    let client = deps
        .worker_service()
        .agent_http_client(&non_shared_user.token)
        .await;

    let result = client
        .invoke_agent(
            Some(&IdempotencyKey::fresh().value),
            &golem_client::model::AgentInvocationRequest {
                app_name: app.name.0.clone(),
                env_name: env.name.0.clone(),
                agent_type_name: agent_id.agent_type.to_wit_naming().0.clone(),
                parameters: agent_id.parameters.clone().into(),
                phantom_id: agent_id.phantom_id,
                method_name: "get_value".to_string(),
                method_parameters: data_value!().into(),
                mode: golem_client::model::AgentInvocationMode::Await,
                schedule_at: None,
                idempotency_key: None,
                deployment_revision: None,
                owner_account_email: Some(owner.account_email.0.clone()),
            },
        )
        .await;

    // Authorization failure is mapped to NotFound to prevent resource enumeration
    assert!(
        matches!(
            result,
            Err(golem_client::Error::Item(AgentError::Error404(_)))
        ),
        "Expected 404 NotFound error, got: {:?}",
        result
    );

    Ok(())
}
