//! MCP OAuth metadata validation and authorization requests. Credential ownership,
//! durable exchange claims, and network admission belong to the calling host.

use crate::transport::{HttpSend, TransportError};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope,
    TokenResponse, TokenUrl,
    basic::{BasicClient, BasicTokenResponse, BasicTokenType},
};
use serde::{Deserialize, Serialize};
use url::Url;

mod network;
pub use network::{Discovery, Limits, discover};

#[derive(Deserialize)]
pub struct ProtectedResourceMetadata {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Option<Vec<String>>,
}

impl ProtectedResourceMetadata {
    pub fn validate(&self, resource: &str, configured_issuer: &str) -> Result<(), TransportError> {
        if self.resource != resource {
            return Err(protocol("OAuth protected resource mismatch"));
        }
        if !self
            .authorization_servers
            .iter()
            .any(|issuer| issuer == configured_issuer)
        {
            return Err(protocol(
                "resource metadata does not bind the configured issuer",
            ));
        }
        https_url(configured_issuer)?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub response_types_supported: Vec<String>,
    pub code_challenge_methods_supported: Option<Vec<String>>,
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: bool,
}

pub struct AuthorizationServer {
    metadata: AuthorizationServerMetadata,
}

pub struct OAuthClient {
    inner: BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>,
    issuer: String,
    requires_callback_issuer: bool,
    resource: String,
}

pub struct AuthorizationRequest {
    pub url: Url,
    pub state: CsrfToken,
    pub verifier: PkceCodeVerifier,
}

impl AuthorizationServer {
    pub fn validate(
        metadata: AuthorizationServerMetadata,
        expected_issuer: &str,
    ) -> Result<Self, TransportError> {
        // RFC 8414 and RFC 9207 require string identity, not URL equivalence.
        if metadata.issuer != expected_issuer {
            return Err(protocol("authorization server issuer mismatch"));
        }
        let issuer = https_url(expected_issuer)?;
        if issuer.query().is_some() {
            return Err(invalid("issuer must not contain a query"));
        }
        let authorization = https_url(&metadata.authorization_endpoint)?;
        https_url(&metadata.token_endpoint)?;
        if authorization.query_pairs().any(|(key, _)| {
            matches!(
                key.as_ref(),
                "response_type"
                    | "client_id"
                    | "redirect_uri"
                    | "scope"
                    | "state"
                    | "code_challenge"
                    | "code_challenge_method"
                    | "resource"
            )
        }) {
            return Err(invalid(
                "authorization endpoint contains reserved parameters",
            ));
        }
        if !metadata
            .response_types_supported
            .iter()
            .any(|kind| kind == "code")
            || !metadata
                .code_challenge_methods_supported
                .as_ref()
                .is_some_and(|methods| methods.iter().any(|method| method == "S256"))
        {
            return Err(invalid(
                "authorization server must advertise code and S256 support",
            ));
        }
        Ok(Self { metadata })
    }

    pub fn issuer(&self) -> &str {
        &self.metadata.issuer
    }

    /// Persist with state and PKCE; callbacks must not re-discover metadata.
    pub fn metadata(&self) -> &AuthorizationServerMetadata {
        &self.metadata
    }

    pub fn requires_callback_issuer(&self) -> bool {
        self.metadata.authorization_response_iss_parameter_supported
    }
    pub fn token_endpoint(&self) -> &str {
        &self.metadata.token_endpoint
    }

    pub fn client(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        callback: &str,
        resource: &str,
    ) -> Result<OAuthClient, TransportError> {
        https_url(resource)?;
        let callback_url = Url::parse(callback).map_err(|_| invalid("invalid OAuth callback"))?;
        if callback.bytes().any(|b| b.is_ascii_control())
            || callback.trim() != callback
            || !(callback_url.scheme() == "https"
                || (callback_url.scheme() == "http"
                    && matches!(
                        callback_url.host_str(),
                        Some("localhost" | "127.0.0.1" | "[::1]")
                    )))
            || callback_url.fragment().is_some()
            || !callback_url.username().is_empty()
            || callback_url.password().is_some()
        {
            return Err(invalid("OAuth callback requires HTTPS or loopback HTTP"));
        }
        let methods = self
            .metadata
            .token_endpoint_auth_methods_supported
            .as_deref();
        let supports = |method: &str| {
            methods.map_or(method == "client_secret_basic", |methods| {
                methods.iter().any(|m| m == method)
            })
        };
        let auth = match client_secret {
            Some(_) if supports("client_secret_basic") => AuthType::BasicAuth,
            Some(_) if supports("client_secret_post") => AuthType::RequestBody,
            None if supports("none") => AuthType::RequestBody,
            _ => return Err(invalid("unsupported token endpoint authentication method")),
        };
        let mut client = BasicClient::new(ClientId::new(client_id.to_owned()))
            .set_auth_uri(
                AuthUrl::new(self.metadata.authorization_endpoint.clone())
                    .map_err(|_| invalid("invalid authorization endpoint"))?,
            )
            .set_token_uri(
                TokenUrl::new(self.metadata.token_endpoint.clone())
                    .map_err(|_| invalid("invalid token endpoint"))?,
            )
            .set_redirect_uri(RedirectUrl::from_url(callback_url))
            .set_auth_type(auth);
        if let Some(secret) = client_secret {
            client = client.set_client_secret(ClientSecret::new(secret.to_owned()));
        }
        Ok(OAuthClient {
            inner: client,
            issuer: self.metadata.issuer.clone(),
            requires_callback_issuer: self.requires_callback_issuer(),
            resource: resource.to_owned(),
        })
    }
}

impl OAuthClient {
    pub fn authorize(&self, scopes: &[String]) -> Result<AuthorizationRequest, TransportError> {
        validate_scopes(scopes)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state) = self
            .inner
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(challenge)
            .add_scopes(scopes.iter().cloned().map(Scope::new))
            .add_extra_param("resource", &self.resource)
            .url();
        Ok(AuthorizationRequest {
            url,
            state,
            verifier,
        })
    }

    /// Call only after the host atomically claims the stored state/PKCE flow.
    /// A failed or cancelled exchange must not be retried without a new grant.
    pub async fn exchange_code<S: HttpSend + Send>(
        &self,
        sender: &mut S,
        code: AuthorizationCode,
        verifier: PkceCodeVerifier,
        received_issuer: Option<&str>,
        limits: Limits,
    ) -> Result<BasicTokenResponse, S::Error>
    where
        S::Error: 'static,
    {
        validate_callback_issuer(&self.issuer, self.requires_callback_issuer, received_issuer)?;
        let adapter = network::TokenSender::new(sender, limits)?;
        let result = self
            .inner
            .exchange_code(code)
            .set_pkce_verifier(verifier)
            .add_extra_param("resource", &self.resource)
            .request_async(&adapter)
            .await
            .map_err(network::exchange_error)?;
        validate_token(&result)?;
        Ok(result)
    }

    /// Call only after claiming refresh ownership in the shared grant store.
    /// Any failure requires reauthorization, never retrying the claimed token.
    pub async fn refresh<S: HttpSend + Send>(
        &self,
        sender: &mut S,
        token: &RefreshToken,
        limits: Limits,
    ) -> Result<BasicTokenResponse, S::Error>
    where
        S::Error: 'static,
    {
        let adapter = network::TokenSender::new(sender, limits)?;
        let result = self
            .inner
            .exchange_refresh_token(token)
            .add_extra_param("resource", &self.resource)
            .request_async(&adapter)
            .await
            .map_err(network::exchange_error)?;
        validate_token(&result)?;
        Ok(result)
    }
}

fn validate_token(token: &BasicTokenResponse) -> Result<(), TransportError> {
    if token.token_type() != &BasicTokenType::Bearer || token.access_token().secret().is_empty() {
        return Err(protocol("OAuth provider did not issue a bearer token"));
    }
    Ok(())
}

fn validate_scopes(scopes: &[String]) -> Result<(), TransportError> {
    if scopes.iter().any(|scope| {
        scope.is_empty()
            || !scope
                .bytes()
                .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
    }) {
        return Err(invalid("invalid OAuth scope"));
    }
    Ok(())
}

/// The callback parser must URL-decode `iss` once, without normalizing it.
pub fn validate_callback_issuer(
    expected: &str,
    required: bool,
    received: Option<&str>,
) -> Result<(), TransportError> {
    match received {
        Some(issuer) if issuer == expected => Ok(()),
        None if !required => Ok(()),
        _ => Err(invalid("OAuth callback issuer mismatch")),
    }
}

pub fn authorization_metadata_urls(issuer: &str) -> Result<Vec<Url>, TransportError> {
    let mut base = https_url(issuer)?;
    if base.query().is_some() {
        return Err(invalid("issuer must not contain a query"));
    }
    let path = base.path().trim_end_matches('/').to_owned();
    base.set_path(&format!("/.well-known/oauth-authorization-server{path}"));
    let mut result = vec![base.clone()];
    base.set_path(&format!("/.well-known/openid-configuration{path}"));
    result.push(base.clone());
    if !path.is_empty() {
        base.set_path(&format!("{path}/.well-known/openid-configuration"));
        result.push(base);
    }
    Ok(result)
}

/// Validate resources and provider URLs before any OAuth network request.
pub fn https_url(value: &str) -> Result<Url, TransportError> {
    let url = Url::parse(value).map_err(|_| invalid("invalid OAuth URL"))?;
    if value.bytes().any(|byte| byte.is_ascii_control())
        || value.trim() != value
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "OAuth URL requires HTTPS without credentials or fragment",
        ));
    }
    Ok(url)
}

fn invalid(message: &str) -> TransportError {
    TransportError::Configuration(message.into())
}

fn protocol(message: &str) -> TransportError {
    TransportError::Protocol(message.into())
}

#[cfg(test)]
mod tests;
