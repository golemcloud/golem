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

//! A read of an exact `(entity ID, revision)` returns the revision that the registry retains, also
//! after the entity is deleted or recreated. A read of the latest or staged entity still hides a
//! deleted entity. A deletion tombstone, a read without the view grant, and a read in a deleted
//! environment or application stay not found.

use golem_client::api::{
    RegistryServiceClient, RegistryServiceGetComponentError,
    RegistryServiceGetComponentRevisionError, RegistryServiceGetComponentWasmError,
    RegistryServiceGetEnvironmentHttpApiDeploymentError,
    RegistryServiceGetEnvironmentMcpDeploymentError, RegistryServiceGetHttpApiDeploymentError,
    RegistryServiceGetHttpApiDeploymentRevisionError, RegistryServiceGetMcpDeploymentError,
    RegistryServiceGetMcpDeploymentRevisionError, RegistryServiceGetResourceError,
    RegistryServiceGetResourceRevisionError,
};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation, HttpApiDeploymentUpdate,
};
use golem_common::model::mcp_deployment::{
    McpDeploymentAgentOptions, McpDeploymentCreation, McpDeploymentUpdate,
};
use golem_common::model::quota::{
    EnforcementAction, ResourceCapacityLimit, ResourceDefinitionCreation, ResourceDefinitionUpdate,
    ResourceLimit, ResourceName,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::{assert_eq, assert_matches};
use std::collections::BTreeMap;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

fn agents<T: Default>(names: &[&str]) -> BTreeMap<AgentTypeName, T> {
    names
        .iter()
        .map(|name| (AgentTypeName(name.to_string()), T::default()))
        .collect()
}

#[test]
#[tracing::instrument]
async fn http_api_deployment_revision_is_readable_after_deletion_and_recreation(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (app, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = HttpApiDeploymentCreation {
        scheme: Default::default(),
        domain,
        agents: agents::<HttpApiDeploymentAgentOptions>(&["test-api"]),
        webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
        openapi_endpoint_prefix: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
    };
    let created = client
        .create_http_api_deployment(&env.id.0, &creation)
        .await?;
    let updated = client
        .update_http_api_deployment(
            &created.id.0,
            &HttpApiDeploymentUpdate {
                scheme: None,
                current_revision: created.revision,
                agents: Some(agents::<HttpApiDeploymentAgentOptions>(&[
                    "test-api",
                    "test-api-2",
                ])),
                webhook_prefix: None,
                openapi_endpoint_prefix: None,
            },
        )
        .await?;
    client
        .delete_http_api_deployment(&updated.id.0, updated.revision.into())
        .await?;
    let tombstone = updated.revision.next()?;

    assert_eq!(
        client
            .get_http_api_deployment_revision(&created.id.0, created.revision.into())
            .await?,
        created
    );
    assert_eq!(
        client
            .get_http_api_deployment_revision(&created.id.0, updated.revision.into())
            .await?,
        updated
    );
    assert_matches!(
        client
            .get_http_api_deployment_revision(&created.id.0, tombstone.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetHttpApiDeploymentRevisionError::Error404(_)
        ))
    );
    assert_matches!(
        client.get_http_api_deployment(&created.id.0).await,
        Err(golem_client::Error::Item(
            RegistryServiceGetHttpApiDeploymentError::Error404(_)
        ))
    );
    assert_matches!(
        client
            .get_environment_http_api_deployment(&env.id.0, &created.domain.0)
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetEnvironmentHttpApiDeploymentError::Error404(_)
        ))
    );

    let recreated = client
        .create_http_api_deployment(&env.id.0, &creation)
        .await?;
    assert_eq!(recreated.id, created.id);
    assert_eq!(
        client
            .get_http_api_deployment_revision(&created.id.0, updated.revision.into())
            .await?,
        updated
    );
    assert_eq!(
        client
            .get_http_api_deployment_revision(&created.id.0, recreated.revision.into())
            .await?,
        recreated
    );

    let other_user = deps.user().await?;
    let other_client = deps.registry_service().client(&other_user.token).await;
    assert_matches!(
        other_client
            .get_http_api_deployment_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetHttpApiDeploymentRevisionError::Error404(_)
        ))
    );

    client
        .delete_application(&app.id.0, app.revision.into())
        .await?;
    assert_matches!(
        client
            .get_http_api_deployment_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetHttpApiDeploymentRevisionError::Error404(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn mcp_deployment_revision_is_readable_after_deletion_and_recreation(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = McpDeploymentCreation {
        domain,
        agents: agents::<McpDeploymentAgentOptions>(&["test-mcp"]),
        tools: BTreeMap::new(),
    };
    let created = client.create_mcp_deployment(&env.id.0, &creation).await?;
    let updated = client
        .update_mcp_deployment(
            &created.id.0,
            &McpDeploymentUpdate {
                current_revision: created.revision,
                agents: Some(agents::<McpDeploymentAgentOptions>(&[
                    "test-mcp",
                    "test-mcp-2",
                ])),
                tools: None,
            },
        )
        .await?;
    client
        .delete_mcp_deployment(&updated.id.0, updated.revision.into())
        .await?;
    let tombstone = updated.revision.next()?;

    assert_eq!(
        client
            .get_mcp_deployment_revision(&created.id.0, created.revision.into())
            .await?,
        created
    );
    assert_eq!(
        client
            .get_mcp_deployment_revision(&created.id.0, updated.revision.into())
            .await?,
        updated
    );
    assert_matches!(
        client
            .get_mcp_deployment_revision(&created.id.0, tombstone.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetMcpDeploymentRevisionError::Error404(_)
        ))
    );
    assert_matches!(
        client.get_mcp_deployment(&created.id.0).await,
        Err(golem_client::Error::Item(
            RegistryServiceGetMcpDeploymentError::Error404(_)
        ))
    );
    assert_matches!(
        client
            .get_environment_mcp_deployment(&env.id.0, &created.domain.0)
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetEnvironmentMcpDeploymentError::Error404(_)
        ))
    );

    let recreated = client.create_mcp_deployment(&env.id.0, &creation).await?;
    assert_eq!(recreated.id, created.id);
    assert_eq!(
        client
            .get_mcp_deployment_revision(&created.id.0, updated.revision.into())
            .await?,
        updated
    );
    assert_eq!(
        client
            .get_mcp_deployment_revision(&created.id.0, recreated.revision.into())
            .await?,
        recreated
    );

    let other_user = deps.user().await?;
    let other_client = deps.registry_service().client(&other_user.token).await;
    assert_matches!(
        other_client
            .get_mcp_deployment_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetMcpDeploymentRevisionError::Error404(_)
        ))
    );

    client
        .delete_mcp_deployment(&recreated.id.0, recreated.revision.into())
        .await?;
    client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;
    assert_matches!(
        client
            .get_mcp_deployment_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetMcpDeploymentRevisionError::Error404(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn resource_definition_revision_is_readable_after_deletion(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (app, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let created = client
        .create_resource(
            &env.id.0,
            &ResourceDefinitionCreation {
                name: ResourceName("retained_resource".to_string()),
                limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
                enforcement_action: EnforcementAction::Throttle,
                unit: "u".to_string(),
                units: "us".to_string(),
            },
        )
        .await?;
    let updated = client
        .update_resource(
            &created.id.0,
            &ResourceDefinitionUpdate {
                current_revision: created.revision,
                limit: Some(ResourceLimit::Capacity(ResourceCapacityLimit {
                    value: 200,
                })),
                enforcement_action: None,
                unit: None,
                units: None,
            },
        )
        .await?;
    client
        .delete_resource(&updated.id.0, updated.revision.into())
        .await?;
    let tombstone = updated.revision.next()?;

    assert_eq!(
        client
            .get_resource_revision(&created.id.0, created.revision.into())
            .await?,
        created
    );
    assert_eq!(
        client
            .get_resource_revision(&created.id.0, updated.revision.into())
            .await?,
        updated
    );
    assert_matches!(
        client
            .get_resource_revision(&created.id.0, tombstone.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetResourceRevisionError::Error404(_)
        ))
    );
    assert_matches!(
        client.get_resource(&created.id.0).await,
        Err(golem_client::Error::Item(
            RegistryServiceGetResourceError::Error404(_)
        ))
    );

    let other_user = deps.user().await?;
    let other_client = deps.registry_service().client(&other_user.token).await;
    assert_matches!(
        other_client
            .get_resource_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetResourceRevisionError::Error404(_)
        ))
    );

    client
        .delete_application(&app.id.0, app.revision.into())
        .await?;
    assert_matches!(
        client
            .get_resource_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetResourceRevisionError::Error404(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_revision_is_readable_after_deletion_and_recreation(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (app, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let created = user
        .component(&env.id, "it_agent_update_v1_release")
        .store()
        .await?;
    let wasm = client
        .get_component_wasm(&created.id.0, created.revision.into())
        .await?;
    client
        .delete_component(&created.id.0, created.revision.into())
        .await?;
    let tombstone = created.revision.next()?;

    assert_eq!(
        client
            .get_component_revision(&created.id.0, created.revision.into())
            .await?,
        created
    );
    assert_eq!(
        client
            .get_component_wasm(&created.id.0, created.revision.into())
            .await?,
        wasm
    );
    assert_matches!(
        client
            .get_component_revision(&created.id.0, tombstone.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentRevisionError::Error404(_)
        ))
    );
    assert_matches!(
        client
            .get_component_wasm(&created.id.0, tombstone.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentWasmError::Error404(_)
        ))
    );
    assert_matches!(
        client.get_component(&created.id.0).await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentError::Error404(_)
        ))
    );

    let recreated = user
        .component(&env.id, "it_agent_update_v1_release")
        .store()
        .await?;
    assert_eq!(recreated.id, created.id);
    assert_eq!(
        client
            .get_component_revision(&created.id.0, created.revision.into())
            .await?,
        created
    );
    assert_eq!(
        client
            .get_component_wasm(&created.id.0, created.revision.into())
            .await?,
        wasm
    );
    assert_eq!(
        client
            .get_component_revision(&created.id.0, recreated.revision.into())
            .await?,
        recreated
    );

    let other_user = deps.user().await?;
    let other_client = deps.registry_service().client(&other_user.token).await;
    assert_matches!(
        other_client
            .get_component_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentRevisionError::Error404(_)
        ))
    );
    assert_matches!(
        other_client
            .get_component_wasm(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentWasmError::Error404(_)
        ))
    );

    client
        .delete_component(&recreated.id.0, recreated.revision.into())
        .await?;
    client
        .delete_application(&app.id.0, app.revision.into())
        .await?;
    assert_matches!(
        client
            .get_component_revision(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentRevisionError::Error404(_)
        ))
    );
    assert_matches!(
        client
            .get_component_wasm(&created.id.0, created.revision.into())
            .await,
        Err(golem_client::Error::Item(
            RegistryServiceGetComponentWasmError::Error404(_)
        ))
    );

    Ok(())
}
