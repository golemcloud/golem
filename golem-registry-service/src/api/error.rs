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

use crate::model::auth::AuthorizationError;
use crate::services::account::AccountError;
use crate::services::application::ApplicationError;
use crate::services::auth::AuthError;
use crate::services::component::ComponentError;
use crate::services::environment::EnvironmentError;
use crate::services::environment_share::EnvironmentShareError;
use crate::services::oauth2::OAuth2Error;
use crate::services::plan::PlanError;
use crate::services::token::TokenError;
use golem_common::SafeDisplay;
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use poem_openapi::ApiResponse;
use poem_openapi::payload::Json;

#[derive(ApiResponse, Debug)]
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

impl ApiError {
    pub fn bad_request(message: String) -> Self {
        Self::BadRequest(Json(ErrorsBody {
            errors: vec![message],
            cause: None,
        }))
    }
}

impl ApiErrorDetails for ApiError {
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

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        match self {
            ApiError::BadRequest(inner) => inner.cause.take(),
            ApiError::NotFound(inner) => inner.cause.take(),
            ApiError::Unauthorized(inner) => inner.cause.take(),
            ApiError::InternalError(inner) => inner.cause.take(),
            ApiError::Forbidden(inner) => inner.cause.take(),
            ApiError::Conflict(inner) => inner.cause.take(),
            ApiError::LimitExceeded(inner) => inner.cause.take(),
        }
    }
}

impl From<AuthorizationError> for ApiError {
    fn from(value: AuthorizationError) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: value.to_string(),
            cause: None,
        }))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: "Internal Error".to_string(),
            cause: Some(value.into()),
        }))
    }
}

impl From<AuthError> for ApiError {
    fn from(value: AuthError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            AuthError::CouldNotAuthenticate => {
                Self::Unauthorized(Json(ErrorBody { error, cause: None }))
            }
            AuthError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("AuthError")),
            })),
        }
    }
}

impl From<AccountError> for ApiError {
    fn from(value: AccountError) -> Self {
        let error = value.to_safe_string();
        match value {
            AccountError::Unauthorized(inner) => inner.into(),

            AccountError::AccountNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            AccountError::ConcurrentUpdate => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                cause: None,
            })),

            AccountError::EmailAlreadyInUse => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            AccountError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("AccountError")),
            })),
        }
    }
}

impl From<ApplicationError> for ApiError {
    fn from(value: ApplicationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ApplicationError::Unauthorized(inner) => inner.into(),

            ApplicationError::ApplicationNotFound(_)
            | ApplicationError::ParentAccountNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            ApplicationError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("ApplicationError")),
            })),
        }
    }
}

impl From<EnvironmentError> for ApiError {
    fn from(value: EnvironmentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentError::EnvironmentNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            EnvironmentError::Unauthorized(inner) => inner.into(),

            EnvironmentError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("EnvironmentError")),
            })),
        }
    }
}

impl From<PlanError> for ApiError {
    fn from(value: PlanError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            PlanError::PlanNotFound(_) => Self::NotFound(Json(ErrorBody { error, cause: None })),

            PlanError::Unauthorized(inner) => inner.into(),

            PlanError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("PlanError")),
            })),
        }
    }
}

impl From<ComponentError> for ApiError {
    fn from(value: ComponentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ComponentError::LimitExceeded { .. } => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                cause: None,
            })),

            ComponentError::AlreadyExists(_) => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            ComponentError::ComponentProcessingError(_)
            | ComponentError::InitialComponentFileNotFound { .. }
            | ComponentError::InvalidFilePath(_)
            | ComponentError::InvalidComponentName { .. }
            | ComponentError::InvalidOplogProcessorPlugin
            | ComponentError::InvalidPluginScope { .. }
            | ComponentError::InvalidCurrentRevision
            | ComponentError::MalformedComponentArchive { .. }
            | ComponentError::PluginInstallationNotFound { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    cause: None,
                }))
            }

            ComponentError::ConcurrentUpdate => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            ComponentError::NotFound
            | ComponentError::ParentEnvironmentNotFound(_)
            | ComponentError::PluginNotFound { .. } => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            ComponentError::Unauthorized(inner) => inner.into(),

            ComponentError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("ComponentError")),
            })),
        }
    }
}

impl From<TokenError> for ApiError {
    fn from(value: TokenError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            TokenError::Unauthorized(inner) => inner.into(),
            TokenError::TokenNotFound(_) | TokenError::ParentAccountNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }
            TokenError::TokenBySecretFound => {
                Self::InternalError(Json(ErrorBody { error, cause: None }))
            }
            TokenError::TokenSecretAlreadyExists => {
                Self::InternalError(Json(ErrorBody { error, cause: None }))
            }
            TokenError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("TokenError")),
            })),
        }
    }
}

impl From<OAuth2Error> for ApiError {
    fn from(value: OAuth2Error) -> Self {
        let error: String = value.to_safe_string();
        match value {
            OAuth2Error::InvalidSession(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                cause: None,
            })),
            OAuth2Error::OAuth2WebflowStateNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }
            OAuth2Error::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("OAuth2Error")),
            })),
        }
    }
}

impl From<EnvironmentShareError> for ApiError {
    fn from(value: EnvironmentShareError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentShareError::ConcurrentModification
            | EnvironmentShareError::ShareForAccountAlreadyExists => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }
            EnvironmentShareError::EnvironmentShareNotFound(_)
            | EnvironmentShareError::ParentEnvironmentNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }
            EnvironmentShareError::Unauthorized(inner) => inner.into(),
            EnvironmentShareError::InternalError(inner) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(inner.context("EnvironmentShareError")),
            })),
        }
    }
}
