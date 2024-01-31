use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::TokenId;
use golem_common::model::AccountId;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use golem_service_base::model::{ErrorBody, ErrorsBody};

use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::token::{TokenService, TokenServiceError};

#[derive(ApiResponse)]
pub enum TokenError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Token not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, TokenError>;

impl From<AuthServiceError> for TokenError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                TokenError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                TokenError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<TokenServiceError> for TokenError {
    fn from(value: TokenServiceError) -> Self {
        match value {
            TokenServiceError::Unauthorized(error) => {
                TokenError::Unauthorized(Json(ErrorBody { error }))
            }
            TokenServiceError::Unexpected(error) => {
                TokenError::InternalError(Json(ErrorBody { error }))
            }
            TokenServiceError::ArgValidation(errors) => {
                TokenError::BadRequest(Json(ErrorsBody { errors }))
            }
            TokenServiceError::UnknownTokenId(_) => TokenError::NotFound(Json(ErrorBody {
                error: "Token not found".to_string(),
            })),
        }
    }
}

pub struct TokenApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub token_service: Arc<dyn TokenService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/accounts", tag = ApiTags::Token)]
impl TokenApi {
    /// Get all tokens
    ///
    /// Gets all created tokens of an account.
    /// The format of each element is the same as the data object in the oauth2 endpoint's response.
    #[oai(path = "/:account_id/tokens", method = "get")]
    async fn get_tokens(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<crate::model::Token>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let result = self.token_service.find(&account_id.0, &auth).await?;
        Ok(Json(result))
    }

    #[allow(unused_variables)]
    #[oai(path = "/:account_id/tokens/:token_id", method = "get")]
    /// Get a specific token
    ///
    /// Gets information about a token given by it's identifier.
    /// The JSON is the same as the data object in the oauth2 endpoint's response.
    async fn get_token(
        &self,
        account_id: Path<AccountId>,
        token_id: Path<TokenId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Token>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let result = self.token_service.get(&token_id.0, &auth).await?;
        Ok(Json(result))
    }

    #[oai(path = "/:account_id/tokens", method = "post")]
    /// Create new token
    ///
    /// Creates a new token with a given expiration date.
    /// The response not only contains the token data but also the secret which can be passed as a bearer token to the Authorization header to the Golem Cloud REST API.
    ///
    // Note that this is the only time this secret is returned!
    async fn post_token(
        &self,
        account_id: Path<AccountId>,
        request: Json<CreateTokenDTO>,
        token: GolemSecurityScheme,
    ) -> Result<Json<UnsafeToken>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self
            .token_service
            .create(&account_id.0, &request.0.expires_at, &auth)
            .await?;
        Ok(Json(response))
    }

    #[allow(unused_variables)]
    #[oai(path = "/:account_id/tokens/:token_id", method = "delete")]
    /// Delete a token
    ///
    /// Deletes a previously created token given by it's identifier.
    async fn delete_token(
        &self,
        account_id: Path<AccountId>,
        token_id: Path<TokenId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteTokenResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        // FIXME account_id check
        self.token_service.delete(&token_id.0, &auth).await?;
        Ok(Json(DeleteTokenResponse {}))
    }
}
