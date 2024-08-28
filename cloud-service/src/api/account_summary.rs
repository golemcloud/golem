use crate::api::ApiTags;
use crate::model::*;
use crate::service::account_summary::{AccountSummaryService, AccountSummaryServiceError};
use crate::service::auth::{AuthService, AuthServiceError};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::recorded_http_api_request;
use golem_service_base::model::ErrorBody;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

#[derive(ApiResponse, Debug, Clone)]
pub enum AccountSummaryError {
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for AccountSummaryError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            AccountSummaryError::Unauthorized(_) => "Unauthorized",
            AccountSummaryError::InternalError(_) => "InternalError",
        }
    }
}

type Result<T> = std::result::Result<T, AccountSummaryError>;

impl From<AuthServiceError> for AccountSummaryError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(_) => {
                AccountSummaryError::Unauthorized(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            AuthServiceError::Internal(_) => AccountSummaryError::InternalError(Json(ErrorBody {
                error: value.to_string(),
            })),
        }
    }
}

impl From<AccountSummaryServiceError> for AccountSummaryError {
    fn from(value: AccountSummaryServiceError) -> Self {
        match value {
            AccountSummaryServiceError::Unauthorized(_) => {
                AccountSummaryError::Unauthorized(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            AccountSummaryServiceError::Internal(_) => {
                AccountSummaryError::InternalError(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
        }
    }
}

pub struct AccountSummaryApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_summary_service: Arc<dyn AccountSummaryService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/admin/accounts", tag = ApiTags::AccountSummary)]
impl AccountSummaryApi {
    #[oai(path = "/", method = "get", operation_id = "get_account_summary")]
    async fn get_account_summary(
        &self,
        skip: Query<i32>,
        limit: Query<i32>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<AccountSummary>>> {
        let record = recorded_http_api_request!("get_account_summary",);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let result = self
                .account_summary_service
                .get(skip.0, limit.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(result))
        };

        record.result(response)
    }

    #[oai(path = "/count", method = "get", operation_id = "get_account_count")]
    async fn get_account_count(&self, token: GolemSecurityScheme) -> Result<Json<i64>> {
        let record = recorded_http_api_request!("get_account_count",);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let result = self
                .account_summary_service
                .count(&auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(result as i64))
        };

        record.result(response)
    }
}
