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
    RegistryServiceClient, RegistryServiceCreateDomainRegistrationError,
    RegistryServiceGetDomainRegistrationError,
    RegistryServiceListEnvironmentDomainRegistrationsError,
};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn register_and_fetch_domain(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let domain = Domain("test1.golem.cloud".to_string());

    let domain_registration = client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    assert_eq!(domain_registration.domain, domain);

    {
        let fetched_domain_registration = client
            .get_domain_registration(&domain_registration.id.0)
            .await?;
        assert_eq!(fetched_domain_registration, domain_registration);
    }

    {
        let result = client
            .list_environment_domain_registrations(&env.id.0)
            .await?;
        assert_eq!(result.values, vec![domain_registration]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_domain(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let domain = client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: Domain("test2.golem.cloud".to_string()),
            },
        )
        .await?;

    client.delete_domain_registrations(&domain.id.0).await?;

    {
        let result = client.get_domain_registration(&domain.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetDomainRegistrationError::Error404(_)
            ))
        ));
    }

    {
        let result = client
            .list_environment_domain_registrations(&env.id.0)
            .await?;
        assert!(result.values.is_empty())
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn other_users_cannot_see_domain(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;
    let client_2 = deps.registry_service().client(&user_2.token).await;

    let domain = client_1
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: Domain("test3.golem.cloud".to_string()),
            },
        )
        .await?;

    {
        let result = client_2.get_domain_registration(&domain.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetDomainRegistrationError::Error404(_)
            ))
        ));
    }

    {
        let result = client_2
            .list_environment_domain_registrations(&env.id.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceListEnvironmentDomainRegistrationsError::Error404(_)
            ))
        ));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn registering_domains_twice_fails(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;

    let (_, env_1) = user_1.app_and_env().await?;
    let (_, env_2) = user_2.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;
    let client_2 = deps.registry_service().client(&user_2.token).await;

    let domain = Domain("test4.golem.cloud".to_string());

    client_1
        .create_domain_registration(
            &env_1.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    let result = client_2
        .create_domain_registration(
            &env_2.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateDomainRegistrationError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn domain_can_be_reused_after_deletion(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;

    let (_, env_1) = user_1.app_and_env().await?;
    let (_, env_2) = user_2.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;
    let client_2 = deps.registry_service().client(&user_2.token).await;

    let domain = Domain("test5.golem.cloud".to_string());

    let domain_registration = client_1
        .create_domain_registration(
            &env_1.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    client_1
        .delete_domain_registrations(&domain_registration.id.0)
        .await?;

    let second_domain_registration = client_2
        .create_domain_registration(
            &env_2.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    assert_eq!(second_domain_registration.domain, domain);

    Ok(())
}
