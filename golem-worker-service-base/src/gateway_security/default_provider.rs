use crate::gateway_security::open_id_client::OpenIdClient;
use crate::gateway_security::*;
use async_trait::async_trait;
use openidconnect::core::{
    CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier, CoreProviderMetadata, CoreResponseType,
    CoreTokenResponse,
};
use openidconnect::{AuthenticationFlow, AuthorizationCode, CsrfToken, IssuerUrl, Nonce, Scope};

pub struct DefaultIdentityProvider {}

#[async_trait]
impl IdentityProvider for DefaultIdentityProvider {
    // To be called during API definition registration to then store them in the database
    async fn get_provider_metadata(
        &self,
        issuer_url: &IssuerUrl,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
        let provide_metadata = CoreProviderMetadata::discover_async(
            issuer_url.clone(),
            openidconnect::reqwest::async_http_client,
        )
        .await
        .map_err(|err| IdentityProviderError::FailedToDiscoverProviderMetadata(err.to_string()))?;

        Ok(provide_metadata)
    }

    // To be called during call_back authentication URL which is a injected URL
    async fn exchange_code_for_tokens(
        &self,
        client: &OpenIdClient,
        code: &AuthorizationCode,
    ) -> Result<CoreTokenResponse, IdentityProviderError> {
        let token_response = client
            .client
            .exchange_code(code.clone())
            .request_async(openidconnect::reqwest::async_http_client)
            .await
            .map_err(|err| IdentityProviderError::FailedToExchangeCodeForTokens(err.to_string()))?;

        Ok(token_response)
    }

    // To be called before getting the authorisation URL
    fn get_client(
        &self,
        security_scheme: &SecuritySchemeWithProviderMetadata,
    ) -> Result<OpenIdClient, IdentityProviderError> {
        let client = CoreClient::from_provider_metadata(
            security_scheme.provider_metadata.clone(),
            security_scheme.security_scheme.client_id().clone(),
            Some(security_scheme.security_scheme.client_secret().clone()),
        )
        .set_redirect_uri(security_scheme.security_scheme.redirect_url().clone());

        Ok(OpenIdClient { client })
    }

    fn get_claims(
        &self,
        client: &OpenIdClient,
        core_token_response: CoreTokenResponse,
        nonce: &Nonce,
    ) -> Result<CoreIdTokenClaims, IdentityProviderError> {
        let id_token_verifier: CoreIdTokenVerifier = client.client.id_token_verifier();

        let id_token_claims: &CoreIdTokenClaims = core_token_response
            .extra_fields()
            .id_token()
            .expect("Server did not return an ID token")
            .claims(&id_token_verifier, nonce)
            .map_err(|err| IdentityProviderError::IdTokenVerificationError(err.to_string()))?;

        Ok(id_token_claims.clone())
    }

    fn get_authorization_url(&self, client: &OpenIdClient, scopes: Vec<Scope>) -> AuthorizationUrl {
        let builder = client.client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );

        let builder = scopes
            .iter()
            .fold(builder, |builder, scope| builder.add_scope(scope.clone()));

        let (auth_url, csrf_state, nonce) = builder.url();

        AuthorizationUrl {
            url: auth_url,
            csrf_state,
            nonce,
        }
    }
}
