// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::custom_api::http_test_context::{
    HttpTestContext, make_test_context_with_files_and_setup,
};
use golem_client::api::RegistryServiceClient;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentAgentSecurity, SecuritySchemeAgentSecurity,
};
use golem_common::model::security_scheme::{
    AuthorizationCodePkceConfig, Provider, SecuritySchemeCreation, SecuritySchemeId,
    SecuritySchemeLogin, SecuritySchemeName,
};
use golem_service_base::custom_api::{pkce_authorization_path, pkce_token_path};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::oidc::{OidcFixture, OidcTokenFailure};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use url::Url;

inherit_test_dep!(EnvBasedTestDependencies);

const FRONTEND_ORIGIN: &str = "https://frontend.example";
const FRONTEND_REDIRECT: &str = "https://frontend.example/callback?existing=1";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

struct PkceContext {
    gateway: HttpTestContext,
    fixture: OidcFixture,
    scheme_id: SecuritySchemeId,
}

async fn context(deps: &EnvBasedTestDependencies) -> anyhow::Result<PkceContext> {
    let fixture = OidcFixture::start().await?;
    let scheme_id = Arc::new(Mutex::new(None));
    let captured_id = scheme_id.clone();
    let issuer = fixture.issuer.to_string();
    let gateway = make_test_context_with_files_and_setup(
        deps,
        vec![(
            AgentTypeName("PrincipalAgent".into()),
            HttpApiDeploymentAgentOptions {
                security: Some(HttpApiDeploymentAgentSecurity::SecurityScheme(
                    SecuritySchemeAgentSecurity {
                        security_scheme: SecuritySchemeName("pkce-fixture".into()),
                    },
                )),
            },
        )],
        "golem_it_agent_sdk_ts",
        "golem-it:agent-sdk-ts",
        golem_common::model::http_api_deployment::HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
        &[],
        move |user, env_id, domain| async move {
            let client = user.deps.registry_service().client(&user.token).await;
            let scheme = client
                .create_security_scheme(
                    &env_id.0,
                    &SecuritySchemeCreation {
                        name: SecuritySchemeName("pkce-fixture".into()),
                        provider_type: Provider::custom("fixture".into(), issuer)
                            .map_err(anyhow::Error::msg)?,
                        client_id: "fixture-client".into(),
                        client_secret: "fixture-secret".into(),
                        redirect_url: format!("https://{}/oidc/callback", domain.0),
                        scopes: vec!["email".into(), "profile".into()],
                        login: SecuritySchemeLogin::AuthorizationCodePkce(
                            AuthorizationCodePkceConfig {
                                redirect_uris: vec![FRONTEND_REDIRECT.into()],
                                origins: vec![FRONTEND_ORIGIN.into()],
                            },
                        ),
                    },
                )
                .await?;
            *captured_id.lock().unwrap() = Some(scheme.id);
            Ok(())
        },
    )
    .await?;
    let scheme_id = scheme_id.lock().unwrap().unwrap();
    Ok(PkceContext {
        gateway,
        fixture,
        scheme_id,
    })
}

fn gateway_url(context: &PkceContext, location: &Url) -> anyhow::Result<Url> {
    Ok(context.gateway.base_url.join(&format!(
        "{}{}",
        location.path(),
        location
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default()
    ))?)
}

fn token_body(code: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("code_verifier", VERIFIER)
        .append_pair("redirect_uri", FRONTEND_REDIRECT)
        .finish()
}

#[test]
#[timeout("180s")]
async fn authorization_code_pkce_full_flow(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let context = context(deps).await?;
    let mut authorize = context
        .gateway
        .base_url
        .join(&pkce_authorization_path(&context.scheme_id))?;
    authorize
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", FRONTEND_REDIRECT)
        .append_pair("state", "frontend-state")
        .append_pair("code_challenge", CHALLENGE)
        .append_pair("code_challenge_method", "S256");
    let start = context.gateway.client.get(authorize.clone()).send().await?;
    assert_eq!(start.status(), reqwest::StatusCode::FOUND);
    assert!(start.headers().get(reqwest::header::SET_COOKIE).is_none());
    let provider_url = Url::parse(start.headers()[reqwest::header::LOCATION].to_str()?)?;

    let provider = OidcFixture::client_without_redirects()
        .get(provider_url)
        .send()
        .await?;
    assert_eq!(provider.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    let callback = Url::parse(provider.headers()[reqwest::header::LOCATION].to_str()?)?;
    let callback_url = gateway_url(&context, &callback)?;
    let callback = context
        .gateway
        .client
        .get(callback_url.clone())
        .send()
        .await?;
    assert_eq!(callback.status(), reqwest::StatusCode::FOUND);
    assert!(
        callback
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .is_none()
    );
    let frontend = Url::parse(callback.headers()[reqwest::header::LOCATION].to_str()?)?;
    assert_eq!(frontend.origin().ascii_serialization(), FRONTEND_ORIGIN);
    assert_eq!(
        frontend
            .query_pairs()
            .find(|(name, _)| name == "state")
            .unwrap()
            .1,
        "frontend-state"
    );
    let code = frontend
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1
        .into_owned();
    let callback_replay = context.gateway.client.get(callback_url).send().await?;
    assert_eq!(callback_replay.status(), reqwest::StatusCode::BAD_REQUEST);

    let token_url = context
        .gateway
        .base_url
        .join(&pkce_token_path(&context.scheme_id))?;
    let token = context
        .gateway
        .client
        .post(token_url.clone())
        .header(reqwest::header::ORIGIN, FRONTEND_ORIGIN)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(token_body(&code))
        .send()
        .await?;
    assert_eq!(token.status(), reqwest::StatusCode::OK);
    assert_eq!(
        token.headers()[reqwest::header::ACCESS_CONTROL_ALLOW_ORIGIN],
        FRONTEND_ORIGIN
    );
    assert!(token.headers().get(reqwest::header::SET_COOKIE).is_none());
    let token: Value = token.json().await?;
    let bearer = token["access_token"].as_str().unwrap();

    let protected = context
        .gateway
        .base_url
        .join("/principal-agent/pkce-user/authed-principal")?;
    let unauthorized = context.gateway.client.get(protected.clone()).send().await?;
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
    let authenticated = context
        .gateway
        .client
        .get(protected)
        .bearer_auth(bearer)
        .send()
        .await?;
    assert_eq!(authenticated.status(), reqwest::StatusCode::OK);
    let principal: Value = authenticated.json().await?;
    assert_eq!(principal["value"]["oidc"]["sub"], "fixture-user");
    assert_eq!(
        principal["value"]["oidc"]["email"],
        "fixture-user@example.com"
    );

    let replay = context
        .gateway
        .client
        .post(token_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(token_body(&code))
        .send()
        .await?;
    assert_eq!(replay.status(), reqwest::StatusCode::BAD_REQUEST);

    let rejected_start = context.gateway.client.get(authorize).send().await?;
    let provider_url = Url::parse(rejected_start.headers()[reqwest::header::LOCATION].to_str()?)?;
    let provider = OidcFixture::client_without_redirects()
        .get(provider_url)
        .send()
        .await?;
    let callback = Url::parse(provider.headers()[reqwest::header::LOCATION].to_str()?)?;
    context
        .fixture
        .fail_next_token(OidcTokenFailure::InvalidSignature);
    let rejected = context
        .gateway
        .client
        .get(gateway_url(&context, &callback)?)
        .send()
        .await?;
    assert_eq!(rejected.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(rejected.headers().get(reqwest::header::LOCATION).is_none());
    drop(context.fixture);
    Ok(())
}
