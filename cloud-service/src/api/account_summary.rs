use std::sync::Arc;

use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::*;
use crate::service::account_summary::{AccountSummaryService, AccountSummaryServiceError};
use crate::service::auth::{AuthService, AuthServiceError};
use golem_service_base::model::ErrorBody;

use cloud_common::auth::GolemSecurityScheme;

#[derive(ApiResponse)]
pub enum AccountSummaryError {
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, AccountSummaryError>;

impl From<AuthServiceError> for AccountSummaryError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                AccountSummaryError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                AccountSummaryError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<AccountSummaryServiceError> for AccountSummaryError {
    fn from(value: AccountSummaryServiceError) -> Self {
        match value {
            AccountSummaryServiceError::Unauthorized(error) => {
                AccountSummaryError::Unauthorized(Json(ErrorBody { error }))
            }
            AccountSummaryServiceError::Unexpected(error) => {
                AccountSummaryError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

pub struct AccountSummaryApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_summary_service: Arc<dyn AccountSummaryService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/admin/accounts", tag = ApiTags::AccountSummary)]
impl AccountSummaryApi {
    #[oai(path = "/", method = "get")]
    async fn get_account_summary(
        &self,
        skip: Query<i32>,
        limit: Query<i32>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<AccountSummary>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let result = self
            .account_summary_service
            .get(skip.0, limit.0, &auth)
            .await?;
        Ok(Json(result))
    }

    #[oai(path = "/count", method = "get")]
    async fn count_account_summary(&self, token: GolemSecurityScheme) -> Result<Json<i64>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let result = self.account_summary_service.count(&auth).await?;
        Ok(Json(result as i64))
    }
}
