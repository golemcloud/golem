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

use super::Tracing;
use assert2::assert;
use golem_client::api::{RegistryServiceClient, RegistryServiceGetDomainRegistrationError, RegistryServiceListEnvironmentDomainRegistrationsError};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use test_r::{inherit_test_dep, test};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn register_and_fetch_domain(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let domain = Domain("test.golem.cloud".to_string());

    let domain_registration = client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone()
            },
        )
        .await?;

    assert!(domain_registration.domain == domain);

    {
        let fetched_domain_registration = client.get_domain_registration(&domain_registration.id.0).await?;
        assert!(fetched_domain_registration == domain_registration);
    }

    {
        let result = client.list_environment_domain_registrations(&env.id.0).await?;
        assert!(result.values == vec![domain_registration]);
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
                domain: Domain("test.golem.cloud".to_string())
            },
        )
        .await?;

    client.delete_domain_registrations(&domain.id.0).await?;

    {
        let result = client.get_domain_registration(&domain.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetDomainRegistrationError::Error404(_)
            )) = result
        );
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
                domain: Domain("test.golem.cloud".to_string())
            },
        )
        .await?;

    {
        let result = client_2.get_domain_registration(&domain.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetDomainRegistrationError::Error404(_)
            )) = result
        );
    }

    {
        let result = client_2.list_environment_domain_registrations(&domain.id.0).await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceListEnvironmentDomainRegistrationsError::Error404(_)
            )) = result
        );
    }

    Ok(())
}
