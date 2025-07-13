// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::bootstrap::Services;
use crate::login::{LoginError, OAuth2Error};
use crate::service::account::AccountError;
use crate::service::account_grant::AccountGrantServiceError;
use crate::service::auth::AuthServiceError;
use crate::service::project::ProjectError;
use crate::service::project_grant::ProjectGrantError;
use crate::service::project_policy::ProjectPolicyError;
use crate::service::token::TokenServiceError;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::SafeDisplay;
use golem_service_base::api::HealthcheckApi;
use golem_service_base::clients::plugin::PluginError;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, OpenApiService};

mod account;
mod account_summary;
mod dto;
mod grant;
mod limits;
mod login;
mod project;
mod project_grant;
mod project_policy;
mod token;

#[derive(ApiResponse, Debug, Clone)]
pub enum ApiError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Forbidden Request
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    /// Entity not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    Conflict(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl ApiError {
    pub fn logins_disabled() -> Self {
        Self::Conflict(Json(ErrorBody {
            error: "Logins are disabled by configuration".to_string(),
        }))
    }

    pub fn limit_exceeded(error: impl SafeDisplay) -> Self {
        Self::Conflict(Json(ErrorBody {
            error: format!(
                "Allowed number of requests exceeded: {}",
                error.to_safe_string()
            ),
        }))
    }

    pub fn bad_request(error: impl Into<String>) -> Self {
        ApiError::BadRequest(Json(ErrorsBody {
            errors: vec![error.into()],
        }))
    }
}

impl TraceErrorKind for ApiError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ApiError::BadRequest(_) => "BadRequest",
            ApiError::NotFound(_) => "NotFound",
            ApiError::Unauthorized(_) => "Unauthorized",
            ApiError::InternalError(_) => "InternalError",
            ApiError::Conflict(_) => "Conflict",
            ApiError::Forbidden(_) => "Forbidden",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            ApiError::BadRequest(_) => true,
            ApiError::NotFound(_) => true,
            ApiError::Unauthorized(_) => true,
            ApiError::InternalError(_) => false,
            ApiError::Forbidden(_) => true,
            ApiError::Conflict(_) => true,
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

impl From<AuthServiceError> for ApiError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. }
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. } => {
                ApiError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<AccountError> for ApiError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::Internal(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::ArgValidation(errors) => {
                ApiError::BadRequest(Json(ErrorsBody { errors }))
            }
            AccountError::AccountNotFound(_) => ApiError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::InternalRepoError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::InternalPlanError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::AuthError(inner) => inner.into(),
        }
    }
}

impl From<TokenServiceError> for ApiError {
    fn from(value: TokenServiceError) -> Self {
        match value {
            TokenServiceError::InternalRepoError(_)
            | TokenServiceError::InternalSecretAlreadyExists { .. } => {
                ApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            TokenServiceError::ArgValidation(errors) => {
                ApiError::BadRequest(Json(ErrorsBody { errors }))
            }
            TokenServiceError::UnknownToken(_) => ApiError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            TokenServiceError::AccountNotFound(_) => ApiError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            TokenServiceError::UnknownTokenState(_) => ApiError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
        }
    }
}

impl From<AccountGrantServiceError> for ApiError {
    fn from(value: AccountGrantServiceError) -> Self {
        match value {
            AccountGrantServiceError::InternalRepoError(_) => {
                ApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AccountGrantServiceError::ArgValidation(errors) => {
                ApiError::BadRequest(Json(ErrorsBody { errors }))
            }
            AccountGrantServiceError::AccountNotFound(_) => {
                ApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            AccountGrantServiceError::InternalAuthError(inner) => inner.into(),
        }
    }
}

impl From<ProjectPolicyError> for ApiError {
    fn from(value: ProjectPolicyError) -> Self {
        match value {
            ProjectPolicyError::InternalRepoError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<LoginError> for ApiError {
    fn from(value: LoginError) -> Self {
        match &value {
            LoginError::UnknownTokenState(_) => Self::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            _ => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<OAuth2Error> for ApiError {
    fn from(value: OAuth2Error) -> Self {
        match value {
            OAuth2Error::InternalGithubClientError(_) | OAuth2Error::InternalSessionError(_) => {
                ApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            OAuth2Error::InvalidSession(_) | OAuth2Error::InvalidState(_) => {
                ApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
        }
    }
}

impl From<ProjectError> for ApiError {
    fn from(value: ProjectError) -> Self {
        match value {
            ProjectError::InternalRepoError(_)
            | ProjectError::FailedToCreateDefaultProject(_)
            | ProjectError::InternalConversionError { .. }
            | ProjectError::InternalPlanLimitError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            ProjectError::LimitExceeded(_) => Self::limit_exceeded(value),
            ProjectError::ProjectNotFound(_) => Self::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            ProjectError::PluginNotFound { .. } => Self::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            ProjectError::InternalPluginError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            ProjectError::CannotDeleteDefaultProject => Self::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            ProjectError::InternalProjectAuthorisationError(inner) => inner.into(),
        }
    }
}

impl From<ProjectGrantError> for ApiError {
    fn from(value: ProjectGrantError) -> Self {
        match value {
            ProjectGrantError::InternalRepoError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            ProjectGrantError::AuthError(inner) => inner.into(),
            ProjectGrantError::ProjectNotFound(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            ProjectGrantError::ProjectPolicyNotFound(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            ProjectGrantError::AccountNotFound(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
        }
    }
}

impl From<PluginError> for ApiError {
    fn from(value: PluginError) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: value.to_safe_string(),
        }))
    }
}

pub type Apis = (
    account::AccountApi,
    account_summary::AccountSummaryApi,
    grant::GrantApi,
    limits::LimitsApi,
    login::LoginApi,
    HealthcheckApi,
    project::ProjectApi,
    project_grant::ProjectGrantApi,
    project_policy::ProjectPolicyApi,
    token::TokenApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            account::AccountApi {
                auth_service: services.auth_service.clone(),
                account_service: services.account_service.clone(),
            },
            account_summary::AccountSummaryApi {
                auth_service: services.auth_service.clone(),
                account_summary_service: services.account_summary_service.clone(),
            },
            grant::GrantApi {
                auth_service: services.auth_service.clone(),
                account_grant_service: services.account_grant_service.clone(),
            },
            limits::LimitsApi {
                auth_service: services.auth_service.clone(),
                plan_limit_service: services.plan_limit_service.clone(),
            },
            login::LoginApi {
                auth_service: services.auth_service.clone(),
                login_system: services.login_system.clone(),
            },
            HealthcheckApi,
            project::ProjectApi {
                auth_service: services.auth_service.clone(),
                project_service: services.project_service.clone(),
                api_mapper: services.api_mapper.clone(),
            },
            project_grant::ProjectGrantApi {
                auth_service: services.auth_service.clone(),
                project_grant_service: services.project_grant_service.clone(),
                project_policy_service: services.project_policy_service.clone(),
                account_service: services.account_service.clone(),
            },
            project_policy::ProjectPolicyApi {
                auth_service: services.auth_service.clone(),
                project_policy_service: services.project_policy_service.clone(),
            },
            token::TokenApi {
                auth_service: services.auth_service.clone(),
                token_service: services.token_service.clone(),
                login_system: services.login_system.clone(),
            },
        ),
        "Golem API",
        "1.0",
    )
}
