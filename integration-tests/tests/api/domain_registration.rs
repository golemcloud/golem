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
    RegistryServiceClient, RegistryServiceCreateDomainRegistrationError,
    RegistryServiceGetDomainRegistrationError,
    RegistryServiceGetEnvironmentDomainRegistrationError,
    RegistryServiceListEnvironmentDomainRegistrationsError,
};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName,
};
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
        let fetched_domain_registration = client
            .get_environment_domain_registration(&env.id.0, &domain.0)
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
async fn granted_domain_view_works_without_environment_view(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let owner_client = deps.registry_service().client(&owner.token).await;
    let (app, env) = owner.app_and_env().await?;

    // Domain registration is globally unique, so this must not collide with any other test's
    // domain (see register_and_fetch_domain and the other_users_* tests).
    let domain = Domain("test6.golem.cloud".to_string());
    let domain_registration = owner_client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    let grantee = deps.user().await?;
    let grantee_client = deps.registry_service().client(&grantee.token).await;

    // Before any grant the by-domain lookup is not visible to the grantee.
    let before = grantee_client
        .get_environment_domain_registration(&env.id.0, &domain.0)
        .await;
    assert!(matches!(
        before,
        Err(golem_client::Error::Item(
            RegistryServiceGetEnvironmentDomainRegistrationError::Error404(_)
        ))
    ));

    // Grant view on this ONE domain registration only — no environment-view permission.
    owner_client
        .create_permission_share(
            &owner.account_id.0,
            &PermissionShareCreation {
                target_account_email: grantee.account_email.clone(),
                name: PermissionShareName("domain-view".to_string()),
                data: PermissionShareData {
                    lower_positive: vec![format!(
                        "environment.domain-registration({}/{}/{}) @ {} : view : {}",
                        owner.account_email.as_str(),
                        app.name.0,
                        env.name.0,
                        grantee.account_email.as_str(),
                        domain.0,
                    )],
                    lower_negative: Vec::new(),
                    upper_positive: Vec::new(),
                    upper_negative: Vec::new(),
                },
            },
        )
        .await?;

    // With only the resource grant the by-domain lookup now succeeds, matching the by-id lookup.
    let fetched = grantee_client
        .get_environment_domain_registration(&env.id.0, &domain.0)
        .await?;
    assert_eq!(fetched, domain_registration);

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

    client.delete_domain_registration(&domain.id.0).await?;

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
            .get_environment_domain_registration(&env.id.0, &domain.domain.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentDomainRegistrationError::Error404(_)
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
        .delete_domain_registration(&domain_registration.id.0)
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
