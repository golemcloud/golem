use async_trait::async_trait;
use openidconnect::core::{CoreClient, CoreIdTokenClaims, CoreTokenResponse};
use openidconnect::{AuthorizationCode, CsrfToken, IssuerUrl, Nonce, Scope};
use url::Url;
use crate::gateway_identity_provider::GolemIdentityProviderMetadata;
use crate::gateway_identity_provider::open_id_client::OpenIdClient;
use crate::gateway_identity_provider::security_scheme::SecurityScheme;

// A high level abstraction of an identity-provider, that
// all providers need to support.
// The workflow is based on the fundamentals of OpenID Connect, and not `openidconnect` crate.
// While abstraction (internally) reuses certain types from `openidconnect`,
// the implementations are not forced to use `openidconnect` crate.
// They mainly exist only for typesafety.
// Provider implementations can reuse the default implementations of this trait, if they want to.

#[async_trait]
pub trait IdentityProvider {
    async fn get_provider_metadata(&self, issuer_url: &IssuerUrl) -> Result<GolemIdentityProviderMetadata, IdentityProviderError>;
    async fn exchange_code_for_tokens(&self, client: &OpenIdClient, code: &AuthorizationCode) -> Result<CoreTokenResponse, IdentityProviderError>;

    fn get_client(&self, provider_metadata: &GolemIdentityProviderMetadata, security_scheme: &SecurityScheme) -> Result<OpenIdClient, IdentityProviderError>;
    fn get_claims(&self, client: &OpenIdClient, core_token_response: CoreTokenResponse, nonce: &Nonce) -> Result<CoreIdTokenClaims, IdentityProviderError>;
    fn get_authorization_url(&self, client: &OpenIdClient, scopes: Vec<Scope>) -> Result<AuthorizationUrl, IdentityProviderError>;
}

pub struct AuthorizationUrl {
    pub url: Url,
    pub csrf_state: CsrfToken,
    pub nonce: Nonce
}

pub enum IdentityProviderError {
    ClientInitError(String),
    InvalidIssuerUrl(String),
    FailedToDiscoverProviderMetadata(String),
    FailedToExchangeCodeForTokens(String),
    IdTokenVerificationError(String)
}

#[derive(Clone)]
pub struct OAuthClient {
    client: CoreClient,
}
