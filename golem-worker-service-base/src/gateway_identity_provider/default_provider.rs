use async_trait::async_trait;
use openidconnect::{AuthenticationFlow, AuthorizationCode, CsrfToken, Nonce, Scope};
use openidconnect::core::{CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier, CoreProviderMetadata, CoreResponseType, CoreTokenResponse};
use tokio::task;
use crate::gateway_identity_provider::identity_provider::{AuthorizationUrl, IdentityProvider, IdentityProviderError};
use crate::gateway_identity_provider::open_id_client::OpenIdClient;
use crate::gateway_identity_provider::security_scheme::SecurityScheme;

pub struct DefaultIdentityProvider {}

#[async_trait]
impl IdentityProvider for DefaultIdentityProvider {
    async fn get_client(&self, security_scheme: &SecurityScheme) -> Result<OpenIdClient, IdentityProviderError> {
        let provider_metadata =
            task::block_in_place(|| {
                CoreProviderMetadata::discover(&security_scheme.issuer_url(), openidconnect::reqwest::http_client)
            }).map_err(|err| {
                IdentityProviderError::FailedToDiscoverProviderMetadata(err.to_string())
            })?;

        let client = CoreClient::from_provider_metadata(provider_metadata, security_scheme.client_id().clone(), Some(security_scheme.client_secret().clone()))
            .set_redirect_uri(security_scheme.redirect_url().clone());

        Ok(OpenIdClient::new(client))
    }

    async fn exchange_code_for_tokens(&self, client: &OpenIdClient, code: &AuthorizationCode) -> Result<CoreTokenResponse, IdentityProviderError> {
        let token_response =
            task::block_in_place(
                || {
                    client.client.exchange_code(code.clone())
                        .request(openidconnect::reqwest::http_client)
                }
            ).map_err(|err| {
                IdentityProviderError::FailedToExchangeCodeForTokens(err.to_string())
            })?;

        Ok(token_response)
    }

    fn get_claims(&self, client: &OpenIdClient, core_token_response: CoreTokenResponse, nonce: &Nonce) -> Result<CoreIdTokenClaims, IdentityProviderError> {
        let id_token_verifier: CoreIdTokenVerifier = client.client.id_token_verifier();

        let id_token_claims: &CoreIdTokenClaims = core_token_response
            .extra_fields()
            .id_token()
            .expect("Server did not return an ID token")
            .claims(&id_token_verifier, nonce)
            .map_err(|err| {
                IdentityProviderError::IdTokenVerificationError(err.to_string())
            })?;

        Ok(id_token_claims.clone())
    }


    fn get_authorization_url(&self, client: &OpenIdClient, scopes: Vec<Scope>) -> Result<AuthorizationUrl, IdentityProviderError> {
        let builder = client
            .client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random
        );

        let builder =
            scopes.iter().fold(builder, |builder, scope| builder.add_scope(scope.clone()));

        let (auth_url, csrf_state, nonce) = builder.url();

        Ok(AuthorizationUrl {
            url: auth_url,
            csrf_state,
            nonce
        })
    }
}


