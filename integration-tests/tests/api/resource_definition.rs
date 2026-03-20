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

use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateResourceError, RegistryServiceDeleteResourceError,
    RegistryServiceGetResourceError, RegistryServiceUpdateResourceError,
};
use golem_common::model::resource_definition::{
    EnforcementAction, ResourceCapacityLimit, ResourceConcurrencyLimit, ResourceDefinitionCreation,
    ResourceDefinitionRevision, ResourceDefinitionUpdate, ResourceLimit, ResourceName,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use pretty_assertions::{assert_eq, assert_matches, assert_ne};
use test_r::inherit_test_dep;
use test_r::test;

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_resource_definition(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("resource_name".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 1000 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "foo".to_string(),
        units: "foos".to_string(),
    };

    let result = client.create_resource(&env.id.0, &creation).await?;

    assert_eq!(result.name, creation.name);
    assert_eq!(result.limit, creation.limit);
    assert_eq!(result.enforcement_action, creation.enforcement_action);
    assert_eq!(result.unit, creation.unit);
    assert_eq!(result.units, creation.units);

    assert_eq!(result.revision, ResourceDefinitionRevision::INITIAL);

    {
        let fetched_secret = client.get_resource(&result.id.0).await?;
        assert_eq!(fetched_secret, result);
    }

    {
        let all_environment_resources = client.get_environment_resources(&env.id.0).await?;
        assert!(all_environment_resources.values.contains(&result));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_create_duplicate_resource_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("dup_resource".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    client.create_resource(&env.id.0, &creation).await?;

    let result = client.create_resource(&env.id.0, &creation).await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateResourceError::Error409(_)
        ))
    );

    Ok(())
}
#[test]
#[tracing::instrument]
async fn update_resource_definition(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("update_resource".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let resource = client.create_resource(&env.id.0, &creation).await?;

    let update = ResourceDefinitionUpdate {
        current_revision: resource.revision,
        limit: Some(ResourceLimit::Capacity(ResourceCapacityLimit {
            value: 200,
        })),
        enforcement_action: None,
        unit: None,
        units: None,
    };

    let updated = client.update_resource(&resource.id.0, &update).await?;

    assert_eq!(updated.id, resource.id);
    assert_eq!(updated.revision, resource.revision.next()?);
    assert_eq!(
        updated.limit,
        ResourceLimit::Capacity(ResourceCapacityLimit { value: 200 })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn update_with_wrong_revision_is_rejected(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("rev_resource".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let resource = client.create_resource(&env.id.0, &creation).await?;

    let wrong_revision = resource.revision.next()?;

    let update = ResourceDefinitionUpdate {
        current_revision: wrong_revision,
        limit: Some(ResourceLimit::Capacity(ResourceCapacityLimit {
            value: 200,
        })),
        enforcement_action: None,
        unit: None,
        units: None,
    };

    let result = client.update_resource(&resource.id.0, &update).await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceUpdateResourceError::Error409(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_resource_definition(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("delete_resource".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let resource = client.create_resource(&env.id.0, &creation).await?;

    client
        .delete_resource(&resource.id.0, resource.revision.into())
        .await?;

    {
        let fetched_resource = client.get_resource(&resource.id.0).await;
        assert_matches!(
            fetched_resource,
            Err(golem_client::Error::Item(
                RegistryServiceGetResourceError::Error404(_)
            ))
        );
    }

    {
        let all_environment_resources = client.get_environment_resources(&env.id.0).await?;
        assert_eq!(all_environment_resources.values, Vec::new());
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_with_wrong_revision_is_rejected(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("delete_rev_resource".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let resource = client.create_resource(&env.id.0, &creation).await?;

    let wrong_revision = resource.revision.next()?;

    let result = client
        .delete_resource(&resource.id.0, wrong_revision.into())
        .await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeleteResourceError::Error409(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deleting_twice_returns_404(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("delete_twice".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let resource = client.create_resource(&env.id.0, &creation).await?;

    client
        .delete_resource(&resource.id.0, resource.revision.into())
        .await?;

    let result = client
        .delete_resource(&resource.id.0, resource.revision.into())
        .await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeleteResourceError::Error404(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn resource_recreation_with_same_type(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("resurrect_me".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let r1 = client.create_resource(&env.id.0, &creation).await?;

    client.delete_resource(&r1.id.0, r1.revision.into()).await?;

    let r2 = client.create_resource(&env.id.0, &creation).await?;

    assert_ne!(r1.id, r2.id);
    assert_eq!(r2.revision, ResourceDefinitionRevision::INITIAL);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn resource_recreation_with_different_types(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let mut creation = ResourceDefinitionCreation {
        name: ResourceName("resurrect_me".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let r1 = client.create_resource(&env.id.0, &creation).await?;

    client.delete_resource(&r1.id.0, r1.revision.into()).await?;

    creation.limit = ResourceLimit::Concurrency(ResourceConcurrencyLimit { value: 100 });

    let r2 = client.create_resource(&env.id.0, &creation).await?;

    assert_ne!(r1.id, r2.id);
    assert_eq!(r2.revision, ResourceDefinitionRevision::INITIAL);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cannot_access_resource_from_another_user(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_a = deps.user().await?;
    let (_, env_a) = user_a.app_and_env().await?;
    let client_a = deps.registry_service().client(&user_a.token).await;

    let creation = ResourceDefinitionCreation {
        name: ResourceName("private_resource".to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "u".to_string(),
        units: "us".to_string(),
    };

    let resource = client_a.create_resource(&env_a.id.0, &creation).await?;

    let user_b = deps.user().await?;
    let client_b = deps.registry_service().client(&user_b.token).await;

    let result = client_b.get_resource(&resource.id.0).await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceGetResourceError::Error404(_)
        ))
    );

    Ok(())
}
