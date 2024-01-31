use std::collections::HashMap;
use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::AccountId;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use golem_service_base::model::{ErrorBody, ErrorsBody};

use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};

#[derive(ApiResponse)]
pub enum LimitsError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, LimitsError>;

impl From<PlanLimitError> for LimitsError {
    fn from(value: PlanLimitError) -> Self {
        match value {
            PlanLimitError::AccountIdNotFound(_) => LimitsError::BadRequest(Json(ErrorsBody {
                errors: vec!["Account not found".to_string()],
            })),
            PlanLimitError::TemplateIdNotFound(_) => LimitsError::BadRequest(Json(ErrorsBody {
                errors: vec!["Template not found".to_string()],
            })),
            PlanLimitError::ProjectIdNotFound(_) => LimitsError::BadRequest(Json(ErrorsBody {
                errors: vec!["Project not found".to_string()],
            })),
            PlanLimitError::Internal(error) => {
                LimitsError::InternalError(Json(ErrorBody { error }))
            }
            PlanLimitError::Unauthorized(error) => {
                LimitsError::Unauthorized(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<AuthServiceError> for LimitsError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                LimitsError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                LimitsError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

pub struct LimitsApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/resource-limits", tag = ApiTags::Limits)]
impl LimitsApi {
    /// Get resource limits for a given account.
    #[oai(path = "/", method = "get")]
    async fn get_resource_limits(
        &self,
        /// The Account ID to check resource limits for.
        #[oai(name = "account-id")]
        account_id: Query<AccountId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResourceLimits>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let result = self
            .plan_limit_service
            .get_resource_limits(&account_id.0, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Update resource limits for a given account.
    #[oai(path = "/", method = "post")]
    async fn update_resource_limits(
        &self,
        limits: Json<BatchUpdateResourceLimits>,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateResourceLimitsResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let mut updates: HashMap<AccountId, i64> = HashMap::new();

        for (k, v) in limits.0.updates.iter() {
            updates.insert(AccountId::from(k.as_str()), *v);
        }

        self.plan_limit_service
            .record_fuel_consumption(updates, &auth)
            .await?;

        Ok(Json(UpdateResourceLimitsResponse {}))
    }
}
