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

use super::{AuthorizationCodeWriteFailureStores, BearerWriteFailureStores};
use chrono::Utc;
use golem_common::model::Empty;
use golem_common::model::component::ComponentId;
use golem_common::model::domain_registration::Domain;
use golem_common::model::security_scheme::{Provider, SecuritySchemeId, SecuritySchemeName};
use golem_service_base::custom_api::{
    CorsOptions, OriginPattern, PathSegment, SecuritySchemeDetails, WebhookCallbackBehaviour,
};
use golem_test_framework::oidc::{OidcFixture, OidcTokenFailure};
use golem_worker_service::custom_api::error::RequestHandlerError;
use golem_worker_service::custom_api::oidc::handler::OidcHandler;
use golem_worker_service::custom_api::oidc::model::{
    AuthorizationCode as GolemAuthorizationCode, AuthorizationUrl, BearerToken, PendingOidcLogin,
    PendingPkceLogin, PkceBinding, SessionId,
};
use golem_worker_service::custom_api::oidc::pkce::{
    MAX_ENCODED_REQUEST_LENGTH, ProtocolErrorCode, TokenResponse,
};
use golem_worker_service::custom_api::oidc::session_store::SessionStore;
use golem_worker_service::custom_api::oidc::{
    DefaultIdentityProvider, IdentityProvider, IdentityProviderError, RawTokenResponse,
};
use golem_worker_service::custom_api::route_resolver::ResolvedRouteEntry;
use golem_worker_service::custom_api::{
    ResponseBody, RichCompiledRoute, RichRequest, RichRouteBehaviour, RichRouteSecurity,
    RichSecuritySchemeRouteSecurity,
};
use openidconnect::core::CoreIdTokenClaims;
use openidconnect::{
    Audience, AuthorizationCode, EmptyAdditionalClaims, IssuerUrl, StandardClaims,
    SubjectIdentifier,
};
use openidconnect::{ClientId, ClientSecret, CsrfToken, Nonce, RedirectUrl, Scope};
use std::str::FromStr;
use std::sync::Arc;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep, timeout};
use url::Url;

inherit_test_dep!(Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("redis")] Arc<dyn SessionStore>);
inherit_test_dep!(#[tagged_as("sqlite")] Arc<dyn SessionStore>);
inherit_test_dep!(
    #[tagged_as("redis_authorization_code_write_failure")]
    AuthorizationCodeWriteFailureStores
);
inherit_test_dep!(
    #[tagged_as("sqlite_authorization_code_write_failure")]
    AuthorizationCodeWriteFailureStores
);
inherit_test_dep!(
    #[tagged_as("redis_bearer_write_failure")]
    BearerWriteFailureStores
);
inherit_test_dep!(
    #[tagged_as("sqlite_bearer_write_failure")]
    BearerWriteFailureStores
);

define_matrix_dimension!(pkce_store: Arc<dyn SessionStore> -> "redis", "sqlite");
define_matrix_dimension!(authorization_code_write_failure: AuthorizationCodeWriteFailureStores -> "redis_authorization_code_write_failure", "sqlite_authorization_code_write_failure");
define_matrix_dimension!(bearer_write_failure: BearerWriteFailureStores -> "redis_bearer_write_failure", "sqlite_bearer_write_failure");

#[test_dep(scope = Shared)]
fn oidc_handler(session_store: &Arc<dyn SessionStore>) -> Arc<OidcHandler> {
    Arc::new(OidcHandler::new(
        session_store.clone(),
        Arc::new(FakeIdentityProvider::default()),
    ))
}

#[derive(Clone, Default)]
struct FakeIdentityProvider {
    fail_exchange: bool,
}

#[async_trait::async_trait]
impl IdentityProvider for FakeIdentityProvider {
    async fn exchange_code_for_scopes_and_claims(
        &self,
        _security_scheme: &SecuritySchemeDetails,
        _code: &AuthorizationCode,
        _nonce: &Nonce,
    ) -> Result<(Vec<Scope>, CoreIdTokenClaims), IdentityProviderError> {
        if self.fail_exchange {
            return Err(IdentityProviderError::OidcTokenExchangeFailed);
        }
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
                "https://fake-idp/auth?state={}&nonce={}",
                csrf_state.secret(),
                nonce.secret()
            ))
            .unwrap(),
            csrf_state,
            nonce,
        })
    }

    async fn exchange_code_for_raw_id_token(
        &self,
        _security_scheme: &SecuritySchemeDetails,
        _code: &AuthorizationCode,
    ) -> Result<RawTokenResponse, IdentityProviderError> {
        Ok(RawTokenResponse {
            id_token: "fake-id-token".to_string(),
            access_token: Some("fake-access-token".to_string()),
            refresh_token: None,
            expires_in: Some(3600),
            token_type: "Bearer".to_string(),
        })
    }

    async fn validate_bearer_token(
        &self,
        _security_scheme: &SecuritySchemeDetails,
        _token: &str,
    ) -> Result<(), IdentityProviderError> {
        Ok(())
    }
}

fn sample_security_scheme() -> Arc<SecuritySchemeDetails> {
    Arc::new(SecuritySchemeDetails {
        id: SecuritySchemeId::new(),
        revision: golem_common::model::security_scheme::SecuritySchemeRevision::INITIAL,
        name: SecuritySchemeName("my-scheme".to_string()),
        provider_type: Provider::Google(Empty {}),
        client_id: ClientId::new("my-client-id".to_string()),
        client_secret: ClientSecret::new("my-client-secret".to_string()),
        redirect_url: RedirectUrl::new("http://example.com/redirect".to_string()).unwrap(),
        scopes: vec![Scope::new("openid".into())],
        login: golem_common::model::security_scheme::SecuritySchemeLogin::Cookie(Empty {}),
    })
}

const FRONTEND_REDIRECT: &str = "https://frontend.example/return?tenant=one";
const FRONTEND_STATE: &str = "frontend state & opaque";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

fn sample_pkce_security_scheme() -> Arc<SecuritySchemeDetails> {
    Arc::new(SecuritySchemeDetails {
        id: SecuritySchemeId::new(),
        revision: golem_common::model::security_scheme::SecuritySchemeRevision::INITIAL,
        name: SecuritySchemeName("my-pkce-scheme".to_string()),
        provider_type: Provider::Google(Empty {}),
        client_id: ClientId::new("my-client-id".to_string()),
        client_secret: ClientSecret::new("my-client-secret".to_string()),
        redirect_url: RedirectUrl::new("http://example.com/oidc/callback".to_string()).unwrap(),
        scopes: vec![Scope::new("openid".into()), Scope::new("email".into())],
        login: golem_common::model::security_scheme::SecuritySchemeLogin::AuthorizationCodePkce(
            golem_common::model::security_scheme::AuthorizationCodePkceConfig {
                redirect_uris: vec![FRONTEND_REDIRECT.to_string()],
                origins: vec!["https://frontend.example".to_string()],
            },
        ),
    })
}

fn pkce_binding(route: &ResolvedRouteEntry, scheme: &SecuritySchemeDetails) -> PkceBinding {
    PkceBinding {
        security_scheme_id: scheme.id,
        security_scheme_revision: scheme.revision,
        environment_id: route.route.environment_id,
        api_origin: Url::parse(&format!(
            "{}://{}",
            route.public_scheme, route.public_authority
        ))
        .unwrap()
        .origin()
        .ascii_serialization(),
    }
}

fn pkce_handler(store: &Arc<dyn SessionStore>, fail_exchange: bool) -> Arc<OidcHandler> {
    Arc::new(OidcHandler::new(
        store.clone(),
        Arc::new(FakeIdentityProvider { fail_exchange }),
    ))
}

async fn fixture_code_and_nonce(
    _fixture: &OidcFixture,
    provider: &DefaultIdentityProvider,
    scheme: &SecuritySchemeDetails,
) -> (AuthorizationCode, Nonce) {
    let authorization = provider
        .get_authorization_url(
            scheme,
            scheme.scopes.clone(),
            CsrfToken::new("upstream-state".into()),
            Nonce::new(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap();
    let response = OidcFixture::client_without_redirects()
        .get(authorization.url)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    let callback =
        Url::parse(response.headers()[http::header::LOCATION].to_str().unwrap()).unwrap();
    let code = callback
        .query_pairs()
        .find_map(|(name, value)| (name == "code").then(|| value.into_owned()))
        .unwrap();
    (AuthorizationCode::new(code), authorization.nonce)
}

#[test]
#[timeout("60s")]
async fn default_identity_provider_verifies_real_http_fixture() {
    let fixture = OidcFixture::start().await.unwrap();
    let scheme = SecuritySchemeDetails {
        id: SecuritySchemeId::new(),
        revision: golem_common::model::security_scheme::SecuritySchemeRevision::INITIAL,
        name: SecuritySchemeName("fixture".into()),
        provider_type: Provider::custom("fixture".into(), fixture.issuer.to_string()).unwrap(),
        client_id: ClientId::new("fixture-client".into()),
        client_secret: ClientSecret::new("fixture-secret".into()),
        redirect_url: RedirectUrl::new("http://gateway.example/callback".into()).unwrap(),
        scopes: vec![Scope::new("email".into())],
        login: golem_common::model::security_scheme::SecuritySchemeLogin::AuthorizationCodePkce(
            golem_common::model::security_scheme::AuthorizationCodePkceConfig {
                redirect_uris: vec![FRONTEND_REDIRECT.into()],
                origins: vec!["https://frontend.example".into()],
            },
        ),
    };
    let provider = DefaultIdentityProvider;
    let (code, nonce) = fixture_code_and_nonce(&fixture, &provider, &scheme).await;
    let (scopes, claims) = provider
        .exchange_code_for_scopes_and_claims(&scheme, &code, &nonce)
        .await
        .unwrap();
    assert_eq!(claims.subject().as_str(), "fixture-user");
    assert!(scopes.iter().any(|scope| scope.as_str() == "email"));

    for failure in [
        OidcTokenFailure::InvalidSignature,
        OidcTokenFailure::InvalidIssuer,
        OidcTokenFailure::InvalidAudience,
        OidcTokenFailure::InvalidNonce,
        OidcTokenFailure::Expired,
        OidcTokenFailure::MissingIdToken,
        OidcTokenFailure::ProviderError,
    ] {
        let (code, nonce) = fixture_code_and_nonce(&fixture, &provider, &scheme).await;
        fixture.fail_next_token(failure);
        assert!(
            provider
                .exchange_code_for_scopes_and_claims(&scheme, &code, &nonce)
                .await
                .is_err(),
            "{failure:?}"
        );
    }
}

#[test]
#[timeout("60s")]
async fn cookie_login_regression_uses_real_provider(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let fixture = OidcFixture::start().await?;
    let scheme = Arc::new(SecuritySchemeDetails {
        id: SecuritySchemeId::new(),
        revision: golem_common::model::security_scheme::SecuritySchemeRevision::INITIAL,
        name: SecuritySchemeName("cookie-fixture".into()),
        provider_type: Provider::custom("fixture".into(), fixture.issuer.to_string())
            .map_err(anyhow::Error::msg)?,
        client_id: ClientId::new("fixture-client".into()),
        client_secret: ClientSecret::new("fixture-secret".into()),
        redirect_url: RedirectUrl::new("http://example.com/oidc/callback".into()).unwrap(),
        scopes: vec![Scope::new("email".into())],
        login: golem_common::model::security_scheme::SecuritySchemeLogin::Cookie(Empty {}),
    });
    let handler = OidcHandler::new(store.clone(), Arc::new(DefaultIdentityProvider));
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let mut protected = request_new("/protected");
    let redirect = handler
        .apply_oidc_incoming_middleware(&mut protected, &route)
        .await?
        .unwrap();
    let provider_url = Url::parse(redirect.headers[http::header::LOCATION].to_str()?)?;
    let provider = OidcFixture::client_without_redirects()
        .get(provider_url)
        .send()
        .await?;
    let callback_url = Url::parse(provider.headers()[http::header::LOCATION].to_str()?)?;
    let mut callback = request_new(&format!(
        "{}?{}",
        callback_url.path(),
        callback_url.query().unwrap()
    ));
    let callback = handler
        .handle_oidc_callback_behaviour(&mut callback, &route, &scheme)
        .await?;
    let cookie = callback.headers[http::header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap();
    let mut authenticated = RichRequest::new(
        poem::Request::builder()
            .uri_str("/protected")
            .header(http::header::COOKIE, cookie)
            .finish(),
    );
    assert!(
        handler
            .apply_oidc_incoming_middleware(&mut authenticated, &route)
            .await?
            .is_none()
    );
    assert!(authenticated.authenticated_session().is_some());
    Ok(())
}

fn authorization_request() -> RichRequest {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", FRONTEND_REDIRECT)
        .append_pair("state", FRONTEND_STATE)
        .append_pair("code_challenge", CHALLENGE)
        .append_pair("code_challenge_method", "S256")
        .finish();
    request_new(&format!("/authorize?{query}"))
}

async fn initiate_pkce(
    handler: &OidcHandler,
    route: &ResolvedRouteEntry,
    scheme: &Arc<SecuritySchemeDetails>,
) -> anyhow::Result<(String, String)> {
    let result = handler
        .handle_pkce_authorization(&authorization_request(), route, scheme)
        .await?;
    assert_eq!(result.status, http::StatusCode::FOUND);
    assert!(!result.headers.contains_key(http::header::SET_COOKIE));
    let location = Url::parse(result.headers[http::header::LOCATION].to_str()?)?;
    let state = location
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    let nonce = location
        .query_pairs()
        .find(|(name, _)| name == "nonce")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    assert_ne!(state, FRONTEND_STATE);
    assert_ne!(nonce, FRONTEND_STATE);
    assert_ne!(state, nonce);
    Ok((state, nonce))
}

async fn issue_code(
    handler: &OidcHandler,
    route: &ResolvedRouteEntry,
    scheme: &Arc<SecuritySchemeDetails>,
) -> anyhow::Result<String> {
    let (upstream_state, _) = initiate_pkce(handler, route, scheme).await?;
    let mut callback = request_new(&format!(
        "/oidc/callback?code=upstream-code&state={upstream_state}"
    ));
    let result = handler
        .handle_oidc_callback_behaviour(&mut callback, route, scheme)
        .await?;
    let frontend = Url::parse(result.headers[http::header::LOCATION].to_str()?)?;
    Ok(frontend
        .query_pairs()
        .find(|(name, _)| name == "code")
        .map(|(_, value)| value.into_owned())
        .unwrap())
}

fn token_request(code: &str, verifier: &str, redirect_uri: &str) -> RichRequest {
    let body = token_form(code, verifier, redirect_uri);
    RichRequest::new(
        poem::Request::builder()
            .uri_str("/token")
            .method(http::Method::POST)
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body),
    )
}

fn token_form(code: &str, verifier: &str, redirect_uri: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("code_verifier", verifier)
        .append_pair("redirect_uri", redirect_uri)
        .finish()
}

async fn token_response(
    result: golem_worker_service::custom_api::RouteExecutionResult,
) -> TokenResponse {
    let ResponseBody::PoemBody { body, .. } = result.body else {
        panic!("expected JSON token response")
    };
    body.into_json().await.unwrap()
}

fn with_bearer(token: &str) -> RichRequest {
    with_authorization(&format!("Bearer {token}"))
}

fn with_authorization(value: &str) -> RichRequest {
    RichRequest::new(
        poem::Request::builder()
            .uri_str("/protected")
            .header(http::header::AUTHORIZATION, value)
            .finish(),
    )
}

fn request_new(path: &str) -> RichRequest {
    RichRequest::new(poem::Request::builder().uri_str(path).finish())
}

pub fn resolved_route_entry_with_oidc(scheme: Arc<SecuritySchemeDetails>) -> ResolvedRouteEntry {
    let compiled_route = RichCompiledRoute {
        account_id: Default::default(),
        account_email: golem_common::model::account::AccountEmail::new("test@golem"),
        environment_id: Default::default(),
        deployment_revision: golem_common::model::deployment::DeploymentRevision::INITIAL,
        route_id: 1,
        route_match: golem_common::model::agent::HttpMethod::Get(golem_common::model::Empty {})
            .into(),
        path: vec![PathSegment::Literal {
            value: "redirect".to_string(),
        }],
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
        public_scheme: "http".to_string(),
        public_authority: "example.com".to_string(),
        route: Arc::new(compiled_route),
        captured_path_parameters: vec![],
        request_target: golem_common::model::agent::http_files::HttpRequestTarget::parse(
            "/redirect",
        )
        .unwrap(),
        openapi_inputs: None,
    }
}

#[test]
async fn oidc_flow_redirect_and_callback(handler: &Arc<OidcHandler>) -> anyhow::Result<()> {
    let scheme = sample_security_scheme();

    let mut request = RichRequest::new(
        poem::Request::builder()
            .uri_str("https://internal.example/protected?x=1&x=%2f+")
            .version(http::Version::HTTP_2)
            .finish(),
    );
    let result = handler
        .apply_oidc_incoming_middleware(
            &mut request,
            &resolved_route_entry_with_oidc(scheme.clone()),
        )
        .await?
        .unwrap();

    assert_eq!(result.status, http::StatusCode::FOUND);
    let redirect_url = result.headers[&http::header::LOCATION].to_str()?;
    assert!(redirect_url.contains("https://fake-idp/auth"));

    let parsed_url = Url::parse(redirect_url)?;
    let state = parsed_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .expect("state parameter missing in redirect URL");

    let mut callback_request = request_new(&format!("/callback?code=testcode&state={}", state));
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let callback_result = handler
        .handle_oidc_callback_behaviour(&mut callback_request, &route, &scheme)
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
    assert_eq!(
        callback_result.headers[&http::header::LOCATION].to_str()?,
        "http://example.com/protected?x=1&x=%2f+"
    );

    Ok(())
}

#[test]
async fn oidc_callback_invalid_state_returns_error(
    handler: &Arc<OidcHandler>,
) -> anyhow::Result<()> {
    let scheme = sample_security_scheme();

    let mut callback_request = request_new("/callback?code=test&state=invalid");
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let err = handler
        .handle_oidc_callback_behaviour(&mut callback_request, &route, &scheme)
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
    let route = resolved_route_entry_with_oidc(scheme_wrong.clone());

    let err = handler
        .handle_oidc_callback_behaviour(&mut callback_request, &route, &scheme_wrong)
        .await
        .unwrap_err();

    matches!(err, RequestHandlerError::OidcSchemeMismatch);
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_authorization_and_callback_issue_bound_code_without_cookie(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);

    let invalid_request = request_new(
        "/authorize?response_type=code&redirect_uri=https%3A%2F%2Fattacker.example%2Freturn&state=state&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256",
    );
    let invalid = handler
        .handle_pkce_authorization(&invalid_request, &route, &scheme)
        .await?;
    assert_eq!(invalid.status, http::StatusCode::BAD_REQUEST);
    assert!(!invalid.headers.contains_key(http::header::LOCATION));

    let (upstream_state, _) = initiate_pkce(&handler, &route, &scheme).await?;

    let mut callback = request_new(&format!(
        "/oidc/callback?code=upstream-code&state={upstream_state}"
    ));
    let result = handler
        .handle_oidc_callback_behaviour(&mut callback, &route, &scheme)
        .await?;
    assert_eq!(result.status, http::StatusCode::FOUND);
    assert!(!result.headers.contains_key(http::header::SET_COOKIE));
    let frontend = Url::parse(result.headers[http::header::LOCATION].to_str()?)?;
    let pairs: Vec<_> = frontend.query_pairs().collect();
    assert!(pairs.contains(&("tenant".into(), "one".into())));
    assert!(pairs.contains(&("state".into(), FRONTEND_STATE.into())));
    let code = pairs
        .iter()
        .find(|(name, _)| name == "code")
        .map(|(_, value)| value.to_string())
        .unwrap();
    assert_ne!(code, "upstream-code");

    let issued = store.take_authorization_code(&code).await?.unwrap();
    assert_eq!(issued.binding, pkce_binding(&route, &scheme));
    assert_eq!(issued.redirect_uri, FRONTEND_REDIRECT);
    assert_eq!(issued.code_challenge, CHALLENGE);
    assert_eq!(issued.principal.subject, "sub");
    assert!(issued.expires_at > Utc::now());
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_denial_is_sanitized_and_preserves_frontend_state(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);
    let (upstream_state, _) = initiate_pkce(&handler, &route, &scheme).await?;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("error", "provider_secret_error")
        .append_pair("error_description", "sensitive provider detail")
        .append_pair("state", &upstream_state)
        .finish();
    let mut callback = request_new(&format!("/oidc/callback?{query}"));
    let result = handler
        .handle_oidc_callback_behaviour(&mut callback, &route, &scheme)
        .await?;
    let location = result.headers[http::header::LOCATION].to_str()?;
    assert_eq!(result.status, http::StatusCode::FOUND);
    assert!(!result.headers.contains_key(http::header::SET_COOKIE));
    assert!(!location.contains("provider_secret_error"));
    assert!(!location.contains("sensitive"));
    let frontend = Url::parse(location)?;
    let pairs: Vec<_> = frontend.query_pairs().collect();
    assert!(pairs.contains(&("tenant".into(), "one".into())));
    assert!(pairs.contains(&("error".into(), "access_denied".into())));
    assert!(pairs.contains(&("state".into(), FRONTEND_STATE.into())));
    assert!(!pairs.iter().any(|(name, _)| name == "code"));
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_callback_is_atomic_and_rejects_unknown_expired_and_changed_scheme(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);

    let mut unknown = request_new("/oidc/callback?code=x&state=unknown");
    assert_eq!(
        handler
            .handle_oidc_callback_behaviour(&mut unknown, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );

    let expired_state = format!("expired-{}", uuid::Uuid::now_v7());
    store
        .store_pending_pkce_login(
            &expired_state,
            PendingPkceLogin {
                binding: pkce_binding(&route, &scheme),
                redirect_uri: FRONTEND_REDIRECT.into(),
                frontend_state: FRONTEND_STATE.into(),
                code_challenge: CHALLENGE.into(),
                upstream_nonce: Nonce::new("nonce".into()),
                expires_at: Utc::now() - chrono::TimeDelta::seconds(1),
            },
        )
        .await?;
    let mut expired = request_new(&format!("/oidc/callback?code=x&state={expired_state}"));
    assert_eq!(
        handler
            .handle_oidc_callback_behaviour(&mut expired, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );

    let (changed_state, _) = initiate_pkce(&handler, &route, &scheme).await?;
    let mut changed_scheme = (*scheme).clone();
    changed_scheme.revision =
        golem_common::model::security_scheme::SecuritySchemeRevision::try_from(1_u64).unwrap();
    let changed_scheme = Arc::new(changed_scheme);
    let mut changed_route = resolved_route_entry_with_oidc(changed_scheme.clone());
    Arc::get_mut(&mut changed_route.route)
        .unwrap()
        .environment_id = route.route.environment_id;
    let original_binding = pkce_binding(&route, &scheme);
    let changed_binding = pkce_binding(&changed_route, &changed_scheme);
    assert_eq!(
        changed_binding.security_scheme_id,
        original_binding.security_scheme_id
    );
    assert_eq!(
        changed_binding.environment_id,
        original_binding.environment_id
    );
    assert_eq!(changed_binding.api_origin, original_binding.api_origin);
    assert_ne!(
        changed_binding.security_scheme_revision,
        original_binding.security_scheme_revision
    );
    let mut changed = request_new(&format!("/oidc/callback?code=x&state={changed_state}"));
    let changed_result = handler
        .handle_oidc_callback_behaviour(&mut changed, &changed_route, &changed_scheme)
        .await?;
    assert_eq!(changed_result.status, http::StatusCode::BAD_REQUEST);
    assert!(!changed_result.headers.contains_key(http::header::LOCATION));

    let (concurrent_state, _) = initiate_pkce(&handler, &route, &scheme).await?;
    let mut first = request_new(&format!(
        "/oidc/callback?code=first&state={concurrent_state}"
    ));
    let mut second = request_new(&format!(
        "/oidc/callback?code=second&state={concurrent_state}"
    ));
    let (first, second) = tokio::join!(
        handler.handle_oidc_callback_behaviour(&mut first, &route, &scheme),
        handler.handle_oidc_callback_behaviour(&mut second, &route, &scheme)
    );
    let mut statuses = vec![first?.status, second?.status];
    statuses.sort();
    assert_eq!(
        statuses,
        vec![http::StatusCode::FOUND, http::StatusCode::BAD_REQUEST]
    );
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_exchange_failure_consumes_state_without_issuing_code(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, true);
    let (state, _) = initiate_pkce(&handler, &route, &scheme).await?;
    let target = format!("/oidc/callback?code=upstream-code&state={state}");
    let mut first = request_new(&target);
    assert!(matches!(
        handler
            .handle_oidc_callback_behaviour(&mut first, &route, &scheme)
            .await,
        Err(RequestHandlerError::OidcTokenExchangeFailed)
    ));
    let mut retry = request_new(&target);
    assert_eq!(
        handler
            .handle_oidc_callback_behaviour(&mut retry, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_code_storage_failure_consumes_state_without_delivering_code(
    #[dimension(authorization_code_write_failure)] stores: &AuthorizationCodeWriteFailureStores,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let healthy_handler = pkce_handler(&stores.healthy, false);
    let failing_handler = pkce_handler(&stores.failing, false);
    let (state, _) = initiate_pkce(&healthy_handler, &route, &scheme).await?;
    let target = format!("/oidc/callback?code=upstream-code&state={state}");
    let mut first = request_new(&target);
    assert!(
        failing_handler
            .handle_oidc_callback_behaviour(&mut first, &route, &scheme)
            .await
            .is_err()
    );
    let mut retry = request_new(&target);
    assert_eq!(
        healthy_handler
            .handle_oidc_callback_behaviour(&mut retry, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_token_exchange_and_bearer_authenticate_without_cookie_fallback(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);
    let code = issue_code(&handler, &route, &scheme).await?;
    let mut request = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    let result = handler
        .handle_pkce_token(&mut request, &route, &scheme)
        .await?;
    assert_eq!(result.status, http::StatusCode::OK);
    assert_eq!(result.headers[http::header::CACHE_CONTROL], "no-store");
    assert_eq!(result.headers[http::header::PRAGMA], "no-cache");
    let response = token_response(result).await;
    assert_eq!(response.token_type, "Bearer");
    assert_eq!(response.expires_in, 3600);
    assert_ne!(response.access_token, code);

    let binding = pkce_binding(&route, &scheme);
    let credential = store
        .get_bearer_credential(&response.access_token, &binding)
        .await?
        .unwrap();
    assert_eq!(credential.principal.subject, "sub");
    let mut authenticated = with_bearer(&response.access_token);
    assert!(
        handler
            .apply_oidc_incoming_middleware(&mut authenticated, &route)
            .await?
            .is_none()
    );
    assert_eq!(
        authenticated.authenticated_session().unwrap().subject,
        "sub"
    );

    let session_id = SessionId(uuid::Uuid::now_v7());
    store
        .store_authenticated_session(&session_id, credential.principal)
        .await?;
    for mut rejected in [
        RichRequest::new(
            poem::Request::builder()
                .uri_str("/protected")
                .header(
                    http::header::COOKIE,
                    format!("golem_session_id={}", session_id.0),
                )
                .header(
                    http::header::AUTHORIZATION,
                    format!("Bearer {}", GolemAuthorizationCode::generate().secret()),
                )
                .finish(),
        ),
        with_bearer("eyJhbGciOiJSUzI1NiJ9.upstream.jwt"),
        with_bearer(GolemAuthorizationCode::generate().secret()),
        with_authorization("Basic dXNlcjpwYXNz"),
        with_authorization("Bearer token with spaces"),
        with_authorization("Bearer short"),
        RichRequest::new(
            poem::Request::builder()
                .uri_str("/protected")
                .header("x-golem-oidc-session", r#"{"subject":"trusted"}"#)
                .finish(),
        ),
    ] {
        let failure = handler
            .apply_oidc_incoming_middleware(&mut rejected, &route)
            .await?
            .unwrap();
        assert_eq!(failure.status, http::StatusCode::UNAUTHORIZED);
        assert_eq!(failure.headers[http::header::WWW_AUTHENTICATE], "Bearer");
        assert!(rejected.authenticated_session().is_none());
    }
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_token_redemption_consumes_code_on_all_proof_failures(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);

    let code = issue_code(&handler, &route, &scheme).await?;
    let mut wrong_content_type = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    wrong_content_type
        .underlying
        .headers_mut()
        .remove(http::header::CONTENT_TYPE);
    let malformed = handler
        .handle_pkce_token(&mut wrong_content_type, &route, &scheme)
        .await?;
    assert_eq!(malformed.status, http::StatusCode::BAD_REQUEST);
    assert_eq!(malformed.headers[http::header::CACHE_CONTROL], "no-store");
    let mut valid_retry = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    assert_eq!(
        handler
            .handle_pkce_token(&mut valid_retry, &route, &scheme)
            .await?
            .status,
        http::StatusCode::OK
    );

    let code = issue_code(&handler, &route, &scheme).await?;
    let mut wrong_verifier = token_request(
        &code,
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        FRONTEND_REDIRECT,
    );
    assert_eq!(
        handler
            .handle_pkce_token(&mut wrong_verifier, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );
    let mut retry = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    assert_eq!(
        handler
            .handle_pkce_token(&mut retry, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );

    let code = issue_code(&handler, &route, &scheme).await?;
    let mut wrong_redirect = token_request(&code, VERIFIER, "https://frontend.example/other");
    assert_eq!(
        handler
            .handle_pkce_token(&mut wrong_redirect, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );
    assert!(store.take_authorization_code(&code).await?.is_none());

    let code = issue_code(&handler, &route, &scheme).await?;
    let mut expired = store.take_authorization_code(&code).await?.unwrap();
    expired.expires_at = Utc::now() - chrono::TimeDelta::seconds(1);
    let expired_code = GolemAuthorizationCode::from_str(&code).unwrap();
    store
        .store_authorization_code(&expired_code, expired)
        .await?;
    let mut expired_request = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    assert_eq!(
        handler
            .handle_pkce_token(&mut expired_request, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );

    let code = issue_code(&handler, &route, &scheme).await?;
    let mut changed_scheme = (*scheme).clone();
    changed_scheme.revision =
        golem_common::model::security_scheme::SecuritySchemeRevision::try_from(1_u64).unwrap();
    let changed_scheme = Arc::new(changed_scheme);
    let mut changed_route = resolved_route_entry_with_oidc(changed_scheme.clone());
    Arc::get_mut(&mut changed_route.route)
        .unwrap()
        .environment_id = route.route.environment_id;
    let mut changed_request = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    assert_eq!(
        handler
            .handle_pkce_token(&mut changed_request, &changed_route, &changed_scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );
    assert!(store.take_authorization_code(&code).await?.is_none());

    let code = issue_code(&handler, &route, &scheme).await?;
    let mut first = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    let mut second = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    let (first, second) = tokio::join!(
        handler.handle_pkce_token(&mut first, &route, &scheme),
        handler.handle_pkce_token(&mut second, &route, &scheme)
    );
    let mut statuses = vec![first?.status, second?.status];
    statuses.sort();
    assert_eq!(
        statuses,
        vec![http::StatusCode::OK, http::StatusCode::BAD_REQUEST]
    );
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_bearer_is_bound_to_current_scheme_environment_and_origin(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);
    let code = issue_code(&handler, &route, &scheme).await?;
    let mut request = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    let token = token_response(
        handler
            .handle_pkce_token(&mut request, &route, &scheme)
            .await?,
    )
    .await
    .access_token;

    let mut changed_scheme = (*scheme).clone();
    changed_scheme.revision =
        golem_common::model::security_scheme::SecuritySchemeRevision::try_from(1_u64).unwrap();
    let changed_scheme = Arc::new(changed_scheme);
    let mut changed_revision = resolved_route_entry_with_oidc(changed_scheme.clone());
    Arc::get_mut(&mut changed_revision.route)
        .unwrap()
        .environment_id = route.route.environment_id;

    let mut changed_environment = resolved_route_entry_with_oidc(scheme.clone());
    changed_environment.public_scheme = route.public_scheme.clone();
    changed_environment.public_authority = route.public_authority.clone();

    let mut changed_origin = resolved_route_entry_with_oidc(scheme.clone());
    Arc::get_mut(&mut changed_origin.route)
        .unwrap()
        .environment_id = route.route.environment_id;
    changed_origin.public_authority = "other.example.com".into();

    let mut other_scheme = (*scheme).clone();
    other_scheme.id = SecuritySchemeId::new();
    let other_scheme = Arc::new(other_scheme);
    let mut changed_scheme_id = resolved_route_entry_with_oidc(other_scheme.clone());
    Arc::get_mut(&mut changed_scheme_id.route)
        .unwrap()
        .environment_id = route.route.environment_id;

    let original_binding = pkce_binding(&route, &scheme);
    let revision_binding = pkce_binding(&changed_revision, &changed_scheme);
    assert_eq!(
        revision_binding.environment_id,
        original_binding.environment_id
    );
    assert_eq!(revision_binding.api_origin, original_binding.api_origin);
    assert_eq!(
        revision_binding.security_scheme_id,
        original_binding.security_scheme_id
    );
    assert_ne!(
        revision_binding.security_scheme_revision,
        original_binding.security_scheme_revision
    );
    let environment_binding = pkce_binding(&changed_environment, &scheme);
    assert_ne!(
        environment_binding.environment_id,
        original_binding.environment_id
    );
    assert_eq!(environment_binding.api_origin, original_binding.api_origin);
    let origin_binding = pkce_binding(&changed_origin, &scheme);
    assert_eq!(
        origin_binding.environment_id,
        original_binding.environment_id
    );
    assert_ne!(origin_binding.api_origin, original_binding.api_origin);
    let scheme_binding = pkce_binding(&changed_scheme_id, &other_scheme);
    assert_eq!(
        scheme_binding.environment_id,
        original_binding.environment_id
    );
    assert_eq!(scheme_binding.api_origin, original_binding.api_origin);
    assert_ne!(
        scheme_binding.security_scheme_id,
        original_binding.security_scheme_id
    );

    for selected in [
        &changed_revision,
        &changed_environment,
        &changed_origin,
        &changed_scheme_id,
    ] {
        let mut request = with_bearer(&token);
        let failure = handler
            .apply_oidc_incoming_middleware(&mut request, selected)
            .await?
            .unwrap();
        assert_eq!(failure.status, http::StatusCode::UNAUTHORIZED);
    }

    let parsed_token = BearerToken::from_str(&token).unwrap();
    let mut credential = store
        .get_bearer_credential(&token, &pkce_binding(&route, &scheme))
        .await?
        .unwrap();
    credential.expires_at = Utc::now() - chrono::TimeDelta::seconds(1);
    store
        .store_bearer_credential(&parsed_token, credential)
        .await?;
    let mut expired = with_bearer(&token);
    assert_eq!(
        handler
            .apply_oidc_incoming_middleware(&mut expired, &route)
            .await?
            .unwrap()
            .status,
        http::StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_bearer_storage_failure_consumes_code_without_delivering_token(
    #[dimension(bearer_write_failure)] stores: &BearerWriteFailureStores,
) -> anyhow::Result<()> {
    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let healthy_handler = pkce_handler(&stores.healthy, false);
    let failing_handler = pkce_handler(&stores.failing, false);
    let code = issue_code(&healthy_handler, &route, &scheme).await?;
    let mut first = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    assert!(
        failing_handler
            .handle_pkce_token(&mut first, &route, &scheme)
            .await
            .is_err()
    );
    let mut retry = token_request(&code, VERIFIER, FRONTEND_REDIRECT);
    assert_eq!(
        healthy_handler
            .handle_pkce_token(&mut retry, &route, &scheme)
            .await?
            .status,
        http::StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[test]
#[timeout("30s")]
async fn pkce_token_stream_stops_reading_at_limit_and_preserves_code(
    #[dimension(pkce_store)] store: &Arc<dyn SessionStore>,
) -> anyhow::Result<()> {
    use bytes::Bytes;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let scheme = sample_pkce_security_scheme();
    let route = resolved_route_entry_with_oidc(scheme.clone());
    let handler = pkce_handler(store, false);
    let code = issue_code(&handler, &route, &scheme).await?;
    let mut oversized = token_form(&code, VERIFIER, FRONTEND_REDIRECT).into_bytes();
    oversized.resize(MAX_ENCODED_REQUEST_LENGTH + 1, b'x');
    let first = Bytes::copy_from_slice(&oversized[..4096]);
    let second = Bytes::copy_from_slice(&oversized[4096..]);
    let polls = Arc::new(AtomicUsize::new(0));
    let stream_polls = polls.clone();
    let body = poem::Body::from_bytes_stream(futures::stream::poll_fn(move |_| {
        let poll = stream_polls.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(match poll {
            0 => Some(Ok::<_, std::io::Error>(first.clone())),
            1 => Some(Ok(second.clone())),
            _ => panic!("token handler polled the stream after crossing its body limit"),
        })
    }));
    let mut request = RichRequest::new(
        poem::Request::builder()
            .uri_str("/token")
            .method(http::Method::POST)
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body),
    );
    let result = handler
        .handle_pkce_token(&mut request, &route, &scheme)
        .await?;
    assert_eq!(result.status, http::StatusCode::BAD_REQUEST);
    assert_eq!(result.headers[http::header::CACHE_CONTROL], "no-store");
    assert_eq!(result.headers[http::header::PRAGMA], "no-cache");
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    let ResponseBody::PoemBody { body, .. } = result.body else {
        panic!("expected protocol error body")
    };
    let error: serde_json::Value = body.into_json().await?;
    assert_eq!(
        error["error"],
        serde_json::json!(ProtocolErrorCode::InvalidRequest)
    );
    assert_eq!(error["error_description"], "request is invalid");
    assert!(store.take_authorization_code(&code).await?.is_some());
    Ok(())
}
