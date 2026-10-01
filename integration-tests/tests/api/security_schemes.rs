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
    RegistryServiceClient, RegistryServiceCreateSecuritySchemeError,
    RegistryServiceGetEnvironmentSecuritySchemeError, RegistryServiceGetSecuritySchemeError,
    RegistryServiceListEnvironmentSecuritySchemesError,
};
use golem_common::model::Empty;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentAgentSecurity, HttpApiDeploymentCreation,
    SecuritySchemeAgentSecurity,
};
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareName,
};
use golem_common::model::security_scheme::{
    AuthorizationCodePkceConfig, Provider, SecuritySchemeCreation, SecuritySchemeLogin,
    SecuritySchemeName, SecuritySchemeUpdate,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::{assert_eq, assert_ne};
use std::collections::BTreeMap;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

fn cookie_login() -> SecuritySchemeLogin {
    SecuritySchemeLogin::Cookie(Empty {})
}

#[test]
#[tracing::instrument]
async fn create_and_fetch_security_scheme(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: SecuritySchemeLogin::AuthorizationCodePkce(AuthorizationCodePkceConfig {
            redirect_uris: vec![
                "https://frontend.example/callback".to_string(),
                "http://127.0.0.1:3000/callback".to_string(),
            ],
            origins: vec![
                "https://frontend.example".to_string(),
                "http://127.0.0.1:3000".to_string(),
            ],
        }),
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
        let fetched_security_scheme = client
            .get_environment_security_scheme(&env.id.0, &security_scheme.name.0)
            .await?;
        assert_eq!(fetched_security_scheme, security_scheme);
    }

    {
        let result = client.list_environment_security_schemes(&env.id.0).await?;
        assert_eq!(result.values, vec![security_scheme]);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn granted_security_scheme_view_works_without_environment_view(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let owner_client = deps.registry_service().client(&owner.token).await;
    let (app, env) = owner.app_and_env().await?;

    let security_scheme_creation = SecuritySchemeCreation {
        name: SecuritySchemeName("test-scheme".to_string()),
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
    };
    let security_scheme = owner_client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    let grantee = deps.user().await?;
    let grantee_client = deps.registry_service().client(&grantee.token).await;

    // Before any grant the by-name lookup is not visible to the grantee.
    let before = grantee_client
        .get_environment_security_scheme(&env.id.0, &security_scheme.name.0)
        .await;
    assert!(matches!(
        before,
        Err(golem_client::Error::Item(
            RegistryServiceGetEnvironmentSecuritySchemeError::Error404(_)
        ))
    ));

    // Grant view on this ONE security scheme only — no environment-view permission.
    owner_client
        .create_permission_share(
            &owner.account_id.0,
            &PermissionShareCreation {
                target_account_email: grantee.account_email.clone(),
                name: PermissionShareName("security-scheme-view".to_string()),
                data: PermissionShareData {
                    lower_positive: vec![format!(
                        "environment.security-scheme({}/{}/{}) @ {} : view : {}",
                        owner.account_email.as_str(),
                        app.name.0,
                        env.name.0,
                        grantee.account_email.as_str(),
                        security_scheme.name.0,
                    )],
                    lower_negative: Vec::new(),
                    upper_positive: Vec::new(),
                    upper_negative: Vec::new(),
                },
            },
        )
        .await?;

    // With only the resource grant the by-name lookup now succeeds, matching the by-id lookup.
    let fetched = grantee_client
        .get_environment_security_scheme(&env.id.0, &security_scheme.name.0)
        .await?;
    assert_eq!(fetched, security_scheme);

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
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
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
        let result = client.list_environment_security_schemes(&env.id.0).await?;
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
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http//example.com".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
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
async fn invalid_pkce_login_configuration_fails_with_bad_request(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let cases = [
        AuthorizationCodePkceConfig {
            redirect_uris: vec![],
            origins: vec!["https://frontend.example".into()],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["https://frontend.example/callback".into()],
            origins: vec![],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["http://frontend.example/callback".into()],
            origins: vec!["http://frontend.example".into()],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["https://user@frontend.example/callback".into()],
            origins: vec!["https://frontend.example".into()],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["https://frontend.example/callback#fragment".into()],
            origins: vec!["https://frontend.example".into()],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["https://frontend.example/callback?state=preseeded".into()],
            origins: vec!["https://frontend.example".into()],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["https://other.example/callback".into()],
            origins: vec!["https://frontend.example".into()],
        },
        AuthorizationCodePkceConfig {
            redirect_uris: vec!["https://frontend.example/callback".into()],
            origins: vec!["https://frontend.example/path".into()],
        },
    ];

    for (index, config) in cases.into_iter().enumerate() {
        let result = client
            .create_security_scheme(
                &env.id.0,
                &SecuritySchemeCreation {
                    name: SecuritySchemeName(format!("invalid-pkce-{index}")),
                    provider_type: Provider::Google(Empty {}),
                    client_id: "client_id".into(),
                    client_secret: "client_secret".into(),
                    redirect_url: "http://localhost:9006/auth/callback".into(),
                    scopes: vec!["openid".into()],
                    login: SecuritySchemeLogin::AuthorizationCodePkce(config),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceCreateSecuritySchemeError::Error400(_)
            ))
        ));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cookie_login_rejects_pkce_fields_in_raw_create_and_update_json(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let service = deps.registry_service();
    let raw_client = service.base_http_client().await;
    let base_url = format!("http://{}:{}", service.http_host(), service.http_port());
    let malformed_login = serde_json::json!({
        "type": "Cookie",
        "redirectUris": ["https://frontend.example/callback"],
        "origins": ["https://frontend.example"]
    });

    let create_response = raw_client
        .post(format!("{base_url}/v1/envs/{}/security-schemes", env.id.0))
        .bearer_auth(user.token.secret())
        .json(&serde_json::json!({
            "name": "malformed-cookie",
            "providerType": { "type": "Google" },
            "clientId": "client_id",
            "clientSecret": "client_secret",
            "redirectUrl": "http://localhost:9006/auth/callback",
            "scopes": ["openid"],
            "login": malformed_login.clone()
        }))
        .send()
        .await?;
    assert_eq!(create_response.status(), reqwest::StatusCode::BAD_REQUEST);

    let typed_client = service.client(&user.token).await;
    let scheme = typed_client
        .create_security_scheme(
            &env.id.0,
            &SecuritySchemeCreation {
                name: SecuritySchemeName("cookie-to-keep".into()),
                provider_type: Provider::Google(Empty {}),
                client_id: "client_id".into(),
                client_secret: "client_secret".into(),
                redirect_url: "http://localhost:9006/auth/callback".into(),
                scopes: vec!["openid".into()],
                login: cookie_login(),
            },
        )
        .await?;
    let update_response = raw_client
        .patch(format!("{base_url}/v1/security-schemes/{}", scheme.id.0))
        .bearer_auth(user.token.secret())
        .json(&serde_json::json!({
            "currentRevision": scheme.revision,
            "login": malformed_login
        }))
        .send()
        .await?;
    assert_eq!(update_response.status(), reqwest::StatusCode::BAD_REQUEST);

    let unchanged = typed_client.get_security_scheme(&scheme.id.0).await?;
    assert_eq!(unchanged.revision, scheme.revision);
    assert_eq!(unchanged.login, cookie_login());

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
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
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
        let result = client_2
            .get_environment_security_scheme(&env.id.0, &security_scheme.name.0)
            .await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceGetEnvironmentSecuritySchemeError::Error404(_)
            ))
        ));
    }

    {
        let result = client_2.list_environment_security_schemes(&env.id.0).await;
        assert!(matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceListEnvironmentSecuritySchemesError::Error404(_)
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
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
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
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
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
        provider_type: Provider::Google(Empty {}),
        client_id: "client_id".to_string(),
        client_secret: "client_secret".to_string(),
        redirect_url: "http://localhost:9006/auth/callback".to_string(),
        scopes: vec!["user".to_string(), "admin".to_string()],
        login: cookie_login(),
    };

    let security_scheme = client
        .create_security_scheme(&env.id.0, &security_scheme_creation)
        .await?;

    let security_scheme_update = SecuritySchemeUpdate {
        current_revision: security_scheme.revision,
        provider_type: Some(Provider::Gitlab(Empty {})),
        client_id: Some("client_id_1".to_string()),
        client_secret: Some("client_secret_1".to_string()),
        redirect_url: Some("http://localhost:9006/auth/callback_1".to_string()),
        scopes: Some(vec!["user_1".to_string(), "admin_1".to_string()]),
        login: Some(SecuritySchemeLogin::AuthorizationCodePkce(
            AuthorizationCodePkceConfig {
                redirect_uris: vec!["https://frontend.example/callback".to_string()],
                origins: vec!["https://frontend.example".to_string()],
            },
        )),
    };

    let updated_security_scheme = client
        .update_security_scheme(&security_scheme.id.0, &security_scheme_update)
        .await?;

    let fetched_updated_security_scheme = client.get_security_scheme(&security_scheme.id.0).await?;

    assert_eq!(fetched_updated_security_scheme, updated_security_scheme);
    assert_eq!(updated_security_scheme.id, security_scheme.id);
    assert_eq!(
        updated_security_scheme.provider_type,
        security_scheme_update.provider_type.unwrap()
    );
    assert_eq!(
        updated_security_scheme.client_id,
        security_scheme_update.client_id.unwrap()
    );
    assert_eq!(
        updated_security_scheme.redirect_url,
        security_scheme_update.redirect_url.unwrap()
    );
    assert_eq!(
        updated_security_scheme.scopes,
        security_scheme_update.scopes.unwrap()
    );
    assert_eq!(
        updated_security_scheme.login,
        security_scheme_update.login.unwrap()
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn security_scheme_update_rejects_collision_with_active_deployment(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let domain = user.register_domain(&env.id).await?;
    let client = deps.registry_service().client(&user.token).await;
    let scheme_name = SecuritySchemeName("active-login".into());
    let security_scheme = client
        .create_security_scheme(
            &env.id.0,
            &SecuritySchemeCreation {
                name: scheme_name.clone(),
                provider_type: Provider::Google(Empty {}),
                client_id: "client_id".into(),
                client_secret: "client_secret".into(),
                redirect_url: format!("https://{}/auth/callback", domain.0),
                scopes: vec!["openid".into()],
                login: cookie_login(),
            },
        )
        .await?;
    user.component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;
    client
        .create_http_api_deployment(
            &env.id.0,
            &HttpApiDeploymentCreation {
                scheme: Default::default(),
                domain: domain.clone(),
                agents: BTreeMap::from([(
                    AgentTypeName("PrincipalAgent".into()),
                    HttpApiDeploymentAgentOptions {
                        security: Some(HttpApiDeploymentAgentSecurity::SecurityScheme(
                            SecuritySchemeAgentSecurity {
                                security_scheme: scheme_name,
                            },
                        )),
                    },
                )]),
                webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
                openapi_endpoint_prefix: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(
                ),
            },
        )
        .await?;
    user.deploy_environment(env.id).await?;

    let result = client
        .update_security_scheme(
            &security_scheme.id.0,
            &SecuritySchemeUpdate {
                current_revision: security_scheme.revision,
                provider_type: None,
                client_id: None,
                client_secret: None,
                redirect_url: Some(format!("https://{}/openapi.json", domain.0)),
                scopes: None,
                login: None,
            },
        )
        .await;
    assert!(result.is_err(), "colliding update unexpectedly succeeded");
    assert_eq!(
        client
            .get_security_scheme(&security_scheme.id.0)
            .await?
            .revision,
        security_scheme.revision,
        "failed validation must not persist a new revision"
    );

    Ok(())
}
