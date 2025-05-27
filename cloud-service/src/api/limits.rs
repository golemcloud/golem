use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::model::AccountId;
use golem_common::recorded_http_api_request;
use golem_common::SafeDisplay;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::Instrument;

#[derive(ApiResponse, Debug, Clone)]
pub enum LimitsError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for LimitsError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            LimitsError::BadRequest(_) => "BadRequest",
            LimitsError::LimitExceeded(_) => "LimitExceeded",
            LimitsError::Unauthorized(_) => "Unauthorized",
            LimitsError::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            LimitsError::BadRequest(_) => true,
            LimitsError::LimitExceeded(_) => true,
            LimitsError::Unauthorized(_) => true,
            LimitsError::InternalError(_) => false,
        }
    }
}

type Result<T> = std::result::Result<T, LimitsError>;

impl From<PlanLimitError> for LimitsError {
    fn from(value: PlanLimitError) -> Self {
        match value {
            PlanLimitError::AccountNotFound(_) => LimitsError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            PlanLimitError::ProjectNotFound(_) => LimitsError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            PlanLimitError::Internal(_) | PlanLimitError::InternalRepoError(_) => {
                LimitsError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            PlanLimitError::LimitExceeded(_) => LimitsError::LimitExceeded(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            PlanLimitError::AuthError(inner) => inner.into(),
        }
    }
}

impl From<AuthServiceError> for LimitsError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. }
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. } => {
                LimitsError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => {
                LimitsError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}

pub struct LimitsApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/resource-limits", tag = ApiTags::Limits)]
impl LimitsApi {
    /// Get resource limits for a given account.
    #[oai(path = "/", method = "get", operation_id = "get_resource_limits")]
    async fn get_resource_limits(
        &self,
        /// The Account ID to check resource limits for.
        #[oai(name = "account-id")]
        account_id: Query<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResourceLimits>> {
        let record = recorded_http_api_request!(
            "get_resource_limits",
            account_id = account_id.0.to_string()
        );
        let response = self
            .get_resource_limits_internal(account_id.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_resource_limits_internal(
        &self,
        account_id: AccountId,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResourceLimits>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let result = self
            .plan_limit_service
            .get_resource_limits(&account_id, &auth)
            .await?;
        Ok(Json(result))
    }

    /// Update resource limits for a given account.
    #[oai(path = "/", method = "post", operation_id = "update_resource_limits")]
    async fn update_resource_limits(
        &self,
        limits: Json<BatchUpdateResourceLimits>,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateResourceLimitsResponse>> {
        let record = recorded_http_api_request!("update_resource_limits",);
        let response = self
            .update_resource_limits_internal(limits.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_resource_limits_internal(
        &self,
        limits: BatchUpdateResourceLimits,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateResourceLimitsResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let mut updates: HashMap<AccountId, i64> = HashMap::new();

        for (k, v) in limits.updates.iter() {
            updates.insert(AccountId::from(k.as_str()), *v);
        }

        self.plan_limit_service
            .record_fuel_consumption(updates, &auth)
            .await?;

        Ok(Json(UpdateResourceLimitsResponse {}))
    }
}
