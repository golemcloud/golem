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

use crate::services::component::ComponentError;
use golem_common::SafeDisplay;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use poem_openapi::ApiResponse;
use poem_openapi::payload::Json;
use crate::services::account::AccountError;
use crate::services::plan::PlanError;
use crate::services::application::ApplicationError;
use crate::services::environment::EnvironmentError;
use crate::services::token::{TokenError, TokenService};

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
    /// Limits of the plan exceeded
    #[oai(status = 422)]
    LimitExceeded(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
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
            ApiError::LimitExceeded(_) => "LimitExceeded",
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
            ApiError::LimitExceeded(_) => true,
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: value.to_string(),
        }))
    }
}

impl From<AccountError> for ApiError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::AccountNotFound(_) => {
                Self::NotFound(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            AccountError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<ApplicationError> for ApiError {
    fn from(value: ApplicationError) -> Self {
        match value {
            ApplicationError::ApplicationNotFound(_) => {
                Self::NotFound(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            ApplicationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<EnvironmentError> for ApiError {
    fn from(value: EnvironmentError) -> Self {
        match value {
            EnvironmentError::EnvironmentNotFound(_) => {
                Self::NotFound(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            EnvironmentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}


impl From<PlanError> for ApiError {
    fn from(value: PlanError) -> Self {
        match value {
            PlanError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<ComponentError> for ApiError {
    fn from(value: ComponentError) -> Self {
        match value {
            ComponentError::Unauthorized(_) => Self::Unauthorized(Json(ErrorBody {
                error: value.to_safe_string(),
            })),

            ComponentError::LimitExceeded { .. } => Self::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),

            ComponentError::AlreadyExists(_) => Self::Conflict(Json(ErrorBody {
                error: value.to_safe_string(),
            })),

            ComponentError::ComponentProcessingError(_)
            | ComponentError::InitialComponentFileNotFound { .. }
            | ComponentError::InvalidFilePath(_)
            | ComponentError::InvalidComponentName { .. }
            | ComponentError::InvalidOplogProcessorPlugin
            | ComponentError::InvalidPluginScope { .. }
            | ComponentError::ConcurrentUpdate { .. }
            | ComponentError::MalformedComponentArchive { .. }
            | ComponentError::PluginInstallationNotFound { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }

            ComponentError::UnknownComponentId(_)
            | ComponentError::UnknownVersionedComponentId(_)
            | ComponentError::PluginNotFound { .. }
            | ComponentError::UnknownEnvironmentComponentName { .. } =>
                Self::NotFound(Json(ErrorBody {
                    error: value.to_safe_string(),
                })),

            ComponentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<TokenError> for ApiError {
    fn from(value: TokenError) -> Self {
        match value {
            TokenError::TokenNotFound(_) => Self::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),

            TokenError::TokenSecretAlreadyExists => Self::InternalError(Json(ErrorBody {
                error: "Internal error".to_string(),
            })),
            TokenError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}
