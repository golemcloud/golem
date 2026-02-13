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
    RegistryServiceClient, RegistryServiceCreateSecuritySchemeError,
    RegistryServiceGetEnvironmentSecuritySchemesError, RegistryServiceGetSecuritySchemeError,
};
use pretty_assertions::{assert_eq, assert_ne};
use golem_common::model::security_scheme::{
    Provider, SecuritySchemeCreation, SecuritySchemeName, SecuritySchemeUpdate,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslExtended;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn create_and_fetch_security_scheme(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    let security_scheme = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    assert_eq!(security_scheme.name, security_scheme_creation.name);

    {
        let fetched_security_scheme = client.get_security_scheme(&security_scheme.id.0).await?;
        assert_eq!(fetched_security_scheme, security_scheme);
    }

    {
        let result = client.get_environment_security_schemes(&env.id.0).await?;
        assert_eq!(result.values, vec![security_scheme]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn delete_security_scheme(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    let security_scheme = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    client
        .delete_security_scheme(&security_scheme.id.0, security_scheme.revision.into())
        .await?;

    {
        let result = client.get_security_scheme(&security_scheme.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetSecuritySchemeError::Error404(_)
            ))
        ));
    }

    {
        let result = client.get_environment_security_schemes(&env.id.0).await?;
        assert!(result.values.is_empty())
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn invalid_redirect_url_fails_with_bad_request(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http//example.com".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    let result = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateSecuritySchemeError::Error400(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn other_users_cannot_see_security_scheme(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let user_2 = deps.user().await?;
    let (_, env) = user_1.app_and_env().await?;

    let client_1 = deps.registry_service().client(&user_1.token).await;
    let client_2 = deps.registry_service().client(&user_2.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    let security_scheme = client_1
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    {
        let result = client_2.get_security_scheme(&security_scheme.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetSecuritySchemeError::Error404(_)
            ))
        ));
    }

    {
        let result = client_2.get_environment_security_schemes(&env.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentSecuritySchemesError::Error404(_)
            ))
        ));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn creating_two_security_schemes_with_same_name_fails(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;

    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    let result = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceCreateSecuritySchemeError::Error409(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn security_scheme_name_can_be_reused_after_deletion(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;

    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    let security_scheme = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    client
        .delete_security_scheme(&security_scheme.id.0, security_scheme.revision.into())
        .await?;

    let recreated_security_scheme = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    assert_eq!(recreated_security_scheme.name, security_scheme.name);
    assert_ne!(recreated_security_scheme.id, security_scheme.id);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn security_scheme_update(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;

    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
    };

    let security_scheme = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    let security_scheme_update = SecuritySchemeUpdate {
        current_revision: security_scheme.revision,
        provider_type: Some(Provider::Gitlab),
        client_id: Some("client_id_1".to_string()),
        client_secret: Some("client_secret_1".to_string()),
        redirect_url: Some("http://localhost:9006/auth/callback_1".to_string()),
        scopes: Some(vec!["user_1".to_string(), "admin_1".to_string()]),
    };

    let updated_security_scheme = client
        .update_security_scheme(&security_scheme.id.0, &security_scheme_update)
        .await?;

    let fetched_updated_security_scheme = client.get_security_scheme(&security_scheme.id.0).await?;

    assert_eq!(fetched_updated_security_scheme, updated_security_scheme);
    assert_eq!(updated_security_scheme.id, security_scheme.id);
    assert_eq!(updated_security_scheme.provider_type, security_scheme_update.provider_type.unwrap());
    assert_eq!(updated_security_scheme.client_id, security_scheme_update.client_id.unwrap());
    assert_eq!(updated_security_scheme.redirect_url, security_scheme_update.redirect_url.unwrap());
    assert_eq!(updated_security_scheme.scopes, security_scheme_update.scopes.unwrap());

    Ok(())
}
