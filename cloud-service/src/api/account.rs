use std::sync::Arc;

use golem_common::model::AccountId;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::*;
use crate::service::account::{AccountError as AccountServiceError, AccountService};
use crate::service::auth::{AuthService, AuthServiceError};
use golem_service_base::model::{ErrorBody, ErrorsBody};

use cloud_common::auth::GolemSecurityScheme;

#[derive(ApiResponse)]
pub enum AccountError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Account not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, AccountError>;

impl From<AuthServiceError> for AccountError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                AccountError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                AccountError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<AccountServiceError> for AccountError {
    fn from(value: AccountServiceError) -> Self {
        match value {
            AccountServiceError::Unauthorized(error) => {
                AccountError::Unauthorized(Json(ErrorBody { error }))
            }
            AccountServiceError::Unexpected(error) => {
                AccountError::InternalError(Json(ErrorBody { error }))
            }
            AccountServiceError::ArgValidation(errors) => {
                AccountError::BadRequest(Json(ErrorsBody { errors }))
            }
            AccountServiceError::UnknownAccountId(account_id) => {
                AccountError::NotFound(Json(ErrorBody {
                    error: format!("Account ID not found {}", account_id.value),
                }))
            }
        }
    }
}

pub struct AccountApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_service: Arc<dyn AccountService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/accounts", tag = ApiTags::Account)]
impl AccountApi {
    /// Get account
    ///
    /// Retrieve an account for a given Account ID
    #[oai(path = "/:account_id", method = "get")]
    async fn get_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Account>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self.account_service.get(&account_id.0, &auth).await?;
        Ok(Json(response))
    }

    /// Get account's plan
    #[oai(path = "/:account_id/plan", method = "get")]
    async fn get_account_plan(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Plan>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self.account_service.get_plan(&account_id.0, &auth).await?;
        Ok(Json(response))
    }

    /// Update account
    ///
    /// Allows the user to change the account details such as name and email.
    ///
    /// Changing the planId is not allowed and the request will be rejected.
    /// The response is the updated account data.
    #[oai(path = "/:account_id", method = "put")]
    async fn put_account(
        &self,
        account_id: Path<AccountId>,
        data: Json<AccountData>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Account>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self
            .account_service
            .update(&account_id.0, &data.0, &auth)
            .await?;
        Ok(Json(response))
    }

    /// Create account
    ///
    /// Create a new account. The response is the created account data.
    #[oai(path = "/", method = "post")]
    async fn post_account(
        &self,
        data: Json<AccountData>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Account>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self
            .account_service
            .create(&AccountId::generate(), &data.0, &auth)
            .await?;
        Ok(Json(response))
    }

    /// Delete account
    ///
    /// Delete an account.
    #[oai(path = "/:account_id", method = "delete")]
    async fn delete_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteAccountResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        self.account_service.delete(&account_id.0, &auth).await?;
        Ok(Json(DeleteAccountResponse {}))
    }
}
