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

use chrono::Utc;
use golem_common::model::component::ComponentId;
use golem_common::model::domain_registration::Domain;
use golem_common::model::security_scheme::{Provider, SecuritySchemeId, SecuritySchemeName};
use golem_service_base::custom_api::{
    CorsOptions, OriginPattern, PathSegment, RequestBodySchema, SecuritySchemeDetails,
    WebhookCallbackBehaviour,
};
use golem_worker_service::custom_api::error::RequestHandlerError;
use golem_worker_service::custom_api::oidc::handler::OidcHandler;
use golem_worker_service::custom_api::oidc::model::{AuthorizationUrl, PendingOidcLogin};
use golem_worker_service::custom_api::oidc::session_store::SessionStore;
use golem_worker_service::custom_api::oidc::{IdentityProvider, IdentityProviderError};
use golem_worker_service::custom_api::route_resolver::ResolvedRouteEntry;
use golem_worker_service::custom_api::{
    RichCompiledRoute, RichRequest, RichRouteBehaviour, RichRouteSecurity,
    RichSecuritySchemeRouteSecurity,
};
use http::Method;
use openidconnect::core::CoreIdTokenClaims;
use openidconnect::{
    Audience, AuthorizationCode, EmptyAdditionalClaims, IssuerUrl, StandardClaims,
    SubjectIdentifier,
};
use openidconnect::{ClientId, ClientSecret, CsrfToken, Nonce, RedirectUrl, Scope};
use std::sync::Arc;
use test_r::{inherit_test_dep, test, test_dep};
use url::Url;

inherit_test_dep!(Arc<dyn SessionStore>);

#[test_dep]
fn oidc_handler(session_store: &Arc<dyn SessionStore>) -> Arc<OidcHandler> {
    Arc::new(OidcHandler::new(
        session_store.clone(),
        Arc::new(FakeIdentityProvider),
    ))
}

#[derive(Clone)]
struct FakeIdentityProvider;

#[async_trait::async_trait]
impl IdentityProvider for FakeIdentityProvider {
    async fn exchange_code_for_scopes_and_claims(
        &self,
        _security_scheme: &SecuritySchemeDetails,
        _code: &AuthorizationCode,
        _nonce: &Nonce,
    ) -> Result<(Vec<Scope>, CoreIdTokenClaims), IdentityProviderError> {
        let issuer = IssuerUrl::new("https://issuer.example".to_string()).unwrap();
        let aud = Audience::new("client_id".to_string());
        let standard_claims = StandardClaims::new(SubjectIdentifier::new("sub".to_string()));

        let claims = CoreIdTokenClaims::new(
            issuer,
            vec![aud],
            Utc::now() + chrono::Duration::hours(8),
            Utc::now(),
            standard_claims,
            EmptyAdditionalClaims {},
        );

        Ok((vec![Scope::new("openid".into())], claims))
    }

    async fn get_authorization_url(
        &self,
        _security_scheme: &SecuritySchemeDetails,
        _scopes: Vec<Scope>,
        csrf_state: CsrfToken,
        nonce: Nonce,
    ) -> Result<AuthorizationUrl, IdentityProviderError> {
        Ok(AuthorizationUrl {
            url: Url::parse(&format!(
                "https://fake-idp/auth?state={}",
                csrf_state.secret()
            ))
            .unwrap(),
            csrf_state,
            nonce,
        })
    }
}

fn sample_security_scheme() -> Arc<SecuritySchemeDetails> {
    Arc::new(SecuritySchemeDetails {
        id: SecuritySchemeId::new(),
        name: SecuritySchemeName("my-scheme".to_string()),
        provider_type: Provider::Google,
        client_id: ClientId::new("my-client-id".to_string()),
        client_secret: ClientSecret::new("my-client-secret".to_string()),
        redirect_url: RedirectUrl::new("http://example.com/redirect".to_string()).unwrap(),
        scopes: vec![Scope::new("openid".into())],
    })
}

fn request_new(path: &str) -> RichRequest {
    RichRequest::new(poem::Request::builder().uri_str(path).finish())
}

pub fn resolved_route_entry_with_oidc(scheme: Arc<SecuritySchemeDetails>) -> ResolvedRouteEntry {
    let compiled_route = RichCompiledRoute {
        account_id: Default::default(),
        environment_id: Default::default(),
        route_id: 1,
        method: Method::GET,
        path: vec![PathSegment::Literal {
            value: "redirect".to_string(),
        }],
        body: RequestBodySchema::Unused,
        behavior: RichRouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
            component_id: ComponentId::new(),
        }),
        security: RichRouteSecurity::SecurityScheme(RichSecuritySchemeRouteSecurity {
            security_scheme: scheme,
        }),
        cors: CorsOptions {
            allowed_patterns: vec![OriginPattern("*".to_string())],
        },
    };

    ResolvedRouteEntry {
        domain: Domain("example.com".to_string()),
        route: Arc::new(compiled_route),
        captured_path_parameters: vec![],
        openapi_spec: None,
    }
}

#[test]
async fn oidc_flow_redirect_and_callback(handler: &Arc<OidcHandler>) -> anyhow::Result<()> {
    let scheme = sample_security_scheme();

    let mut request = request_new("/protected");
    let result = handler
        .apply_oidc_incoming_middleware(
            &mut request,
            &resolved_route_entry_with_oidc(scheme.clone()),
        )
        .await?
        .unwrap();

    assert_eq!(result.status, http::StatusCode::FOUND);
    let redirect_url = result.headers[&http::header::LOCATION].to_string();
    assert!(redirect_url.contains("https://fake-idp/auth"));

    let parsed_url = Url::parse(&redirect_url)?;
    let state = parsed_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .expect("state parameter missing in redirect URL");

    let mut callback_request = request_new(&format!("/callback?code=testcode&state={}", state));
    let callback_result = handler
        .handle_oidc_callback_behaviour(&mut callback_request, &scheme)
        .await?;

    assert_eq!(callback_result.status, http::StatusCode::FOUND);
    assert!(
        callback_result
            .headers
            .contains_key(&http::header::SET_COOKIE)
    );
    assert!(
        callback_result
            .headers
            .contains_key(&http::header::LOCATION)
    );

    Ok(())
}

#[test]
async fn oidc_callback_invalid_state_returns_error(
    handler: &Arc<OidcHandler>,
) -> anyhow::Result<()> {
    let scheme = sample_security_scheme();

    let mut callback_request = request_new("/callback?code=test&state=invalid");
    let err = handler
        .handle_oidc_callback_behaviour(&mut callback_request, &scheme)
        .await
        .unwrap_err();

    assert!(matches!(err, RequestHandlerError::UnknownOidcState));
    Ok(())
}

#[test]
#[allow(clippy::borrowed_box)]
async fn oidc_callback_scheme_mismatch_returns_error(
    handler: &Arc<OidcHandler>,
    store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme_correct = sample_security_scheme();
    let scheme_wrong = sample_security_scheme();

    let pending_login = PendingOidcLogin {
        scheme_id: scheme_correct.id,
        original_uri: "/original".to_string(),
        nonce: Nonce::new("nonce123".into()),
    };

    store
        .store_pending_oidc_login("state1", pending_login)
        .await?;

    let mut callback_request = request_new("/callback?code=test&state=state1");

    let err = handler
        .handle_oidc_callback_behaviour(&mut callback_request, &scheme_wrong)
        .await
        .unwrap_err();

    matches!(err, RequestHandlerError::OidcSchemeMismatch);
    Ok(())
}
