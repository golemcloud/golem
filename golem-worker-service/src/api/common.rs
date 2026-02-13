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

use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::route_resolver::RouteResolverError;
use crate::service::auth::AuthServiceError;
use crate::service::component::ComponentServiceError;
use crate::service::limit::LimitServiceError;
use crate::service::worker::{CallWorkerExecutorError, WorkerServiceError};
use golem_common::SafeDisplay;
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::error::ErrorBody;
use golem_common::model::error::ErrorsBody;
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthorizationError;
use poem_openapi::ApiResponse;
use poem_openapi::payload::Json;
use serde::{Deserialize, Serialize};

/// Detail in case the error was caused by the worker failing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
pub struct WorkerErrorDetails {
    /// Error that caused to worker to fail
    pub cause: String,
    /// Error log of the worker
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
pub struct ErrorBodyWithOptionalWorkerError {
    pub error: String,
    pub worker_error: Option<WorkerErrorDetails>,
}

#[derive(ApiResponse, Debug)]
pub enum ApiEndpointError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    Conflict(Json<ErrorBody>),
    #[oai(status = 422)]
    LimitExceeded(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBodyWithOptionalWorkerError>),
}

impl ApiErrorDetails for ApiEndpointError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            Self::BadRequest(_) => "BadRequest",
            Self::Unauthorized(_) => "Unauthorized",
            Self::Forbidden(_) => "Forbidden",
            Self::NotFound(_) => "NotFound",
            Self::Conflict(_) => "Conflict",
            Self::LimitExceeded(_) => "LimitExceeded",
            Self::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            Self::BadRequest(_) => true,
            Self::NotFound(_) => true,
            Self::Conflict(_) => true,
            Self::LimitExceeded(_) => true,
            Self::Forbidden(_) => true,
            Self::Unauthorized(_) => true,
            Self::InternalError(_) => false,
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        match self {
            Self::BadRequest(inner) => inner.cause.take(),
            Self::NotFound(inner) => inner.cause.take(),
            Self::Unauthorized(inner) => inner.cause.take(),
            Self::InternalError(_) => None,
            Self::Forbidden(inner) => inner.cause.take(),
            Self::LimitExceeded(inner) => inner.cause.take(),
            Self::Conflict(inner) => inner.cause.take(),
        }
    }
}

impl ApiEndpointError {
    pub fn unauthorized<T: SafeDisplay>(error: T) -> Self {
        Self::Unauthorized(Json(ErrorBody {
            error: error.to_safe_string(),
            cause: None,
        }))
    }

    pub fn forbidden<T: SafeDisplay>(error: T) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: error.to_safe_string(),
            cause: None,
        }))
    }

    pub fn internal<T: SafeDisplay>(error: T) -> Self {
        Self::InternalError(Json(ErrorBodyWithOptionalWorkerError {
            error: error.to_safe_string(),
            worker_error: None,
        }))
    }

    pub fn bad_request<T: SafeDisplay>(error: T) -> Self {
        Self::BadRequest(Json(ErrorsBody {
            errors: vec![error.to_safe_string()],
            cause: None,
        }))
    }

    pub fn not_found<T: SafeDisplay>(error: T) -> Self {
        Self::NotFound(Json(ErrorBody {
            error: error.to_safe_string(),
            cause: None,
        }))
    }

    pub fn conflict<T: SafeDisplay>(error: T) -> Self {
        Self::Conflict(Json(ErrorBody {
            error: error.to_safe_string(),
            cause: None,
        }))
    }

    pub fn limit_exceeded<T: SafeDisplay>(error: T) -> Self {
        Self::LimitExceeded(Json(ErrorBody {
            error: error.to_safe_string(),
            cause: None,
        }))
    }
}

impl From<WorkerServiceError> for ApiEndpointError {
    fn from(error: WorkerServiceError) -> Self {
        match error {
            WorkerServiceError::Internal(_) => Self::internal(error),

            WorkerServiceError::FileNotFound(_) => Self::not_found(error),

            WorkerServiceError::TypeChecker(_) | WorkerServiceError::BadFileType(_) => {
                Self::bad_request(error)
            }

            WorkerServiceError::ComponentNotFound(_)
            | WorkerServiceError::AccountIdNotFound(_)
            | WorkerServiceError::WorkerNotFound(_) => Self::not_found(error),

            WorkerServiceError::GolemError(inner) => inner.into(),
            WorkerServiceError::Component(inner) => inner.into(),
            WorkerServiceError::InternalCallError(inner) => inner.into(),
            WorkerServiceError::LimitError(inner) => inner.into(),
            WorkerServiceError::AuthError(inner) => inner.into(),
            WorkerServiceError::RegistryServiceError(inner) => inner.into(),
        }
    }
}

impl From<ComponentServiceError> for ApiEndpointError {
    fn from(error: ComponentServiceError) -> Self {
        match error {
            ComponentServiceError::ComponentNotFound => Self::not_found(error),
            ComponentServiceError::InternalError(_) => Self::internal(error),
        }
    }
}

impl From<LimitServiceError> for ApiEndpointError {
    fn from(error: LimitServiceError) -> Self {
        match error {
            LimitServiceError::LimitExceeded(_) => Self::limit_exceeded(error),
            LimitServiceError::InternalError(_) => Self::internal(error),
        }
    }
}

impl From<CallWorkerExecutorError> for ApiEndpointError {
    fn from(error: CallWorkerExecutorError) -> Self {
        match error {
            CallWorkerExecutorError::FailedToConnectToPod(_) => Self::internal(error),
            CallWorkerExecutorError::FailedToGetRoutingTable(_) => Self::internal(error),
        }
    }
}

impl From<WorkerExecutorError> for ApiEndpointError {
    fn from(error: WorkerExecutorError) -> Self {
        match error {
            WorkerExecutorError::WorkerNotFound { .. } => Self::not_found(error),
            WorkerExecutorError::InvocationFailed { error, stderr } => {
                Self::InternalError(Json(ErrorBodyWithOptionalWorkerError {
                    error: "Invocation Failed".to_string(),
                    worker_error: Some(WorkerErrorDetails {
                        cause: error.message().to_string(),
                        stderr,
                    }),
                }))
            }
            WorkerExecutorError::PreviousInvocationFailed { error, stderr } => {
                Self::InternalError(Json(ErrorBodyWithOptionalWorkerError {
                    error: "Previous Invocation Failed".to_string(),
                    worker_error: Some(WorkerErrorDetails {
                        cause: error.message().to_string(),
                        stderr,
                    }),
                }))
            }
            _ => Self::internal(error),
        }
    }
}

impl From<AuthorizationError> for ApiEndpointError {
    fn from(value: AuthorizationError) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: value.to_string(),
            cause: None,
        }))
    }
}

impl From<AuthServiceError> for ApiEndpointError {
    fn from(error: AuthServiceError) -> Self {
        match error {
            AuthServiceError::Unauthorized(inner) => inner.into(),
            AuthServiceError::CouldNotAuthenticate => Self::unauthorized(error),
            AuthServiceError::InternalError(_) => Self::internal(error),
        }
    }
}

impl From<RegistryServiceError> for ApiEndpointError {
    fn from(value: RegistryServiceError) -> Self {
        match value {
            RegistryServiceError::BadRequest(_) => Self::bad_request(value),
            RegistryServiceError::Unauthorized(_) => Self::unauthorized(value),
            RegistryServiceError::LimitExceeded(_) => Self::limit_exceeded(value),
            RegistryServiceError::NotFound(_) => Self::not_found(value),
            RegistryServiceError::AlreadyExists(_) => Self::conflict(value),
            RegistryServiceError::InternalServerError(_) => Self::internal(value),
            RegistryServiceError::CouldNotAuthenticate(_) => Self::unauthorized(value),
            RegistryServiceError::InternalClientError(_) => Self::internal(value),
        }
    }
}

impl From<RequestHandlerError> for ApiEndpointError {
    fn from(value: RequestHandlerError) -> Self {
        match value {
            RequestHandlerError::ValueParsingFailed { .. }
            | RequestHandlerError::MissingValue { .. }
            | RequestHandlerError::TooManyValues { .. }
            | RequestHandlerError::HeaderIsNotAscii { .. }
            | RequestHandlerError::BodyIsNotValidJson { .. }
            | RequestHandlerError::JsonBodyParsingFailed { .. }
            | RequestHandlerError::UnsupportedMimeType { .. }
            | RequestHandlerError::ResolvingRouteFailed(
                RouteResolverError::CouldNotGetDomainFromRequest(_)
                | RouteResolverError::MalformedPath(_),
            ) => Self::bad_request(value),

            RequestHandlerError::OidcSchemeMismatch => Self::conflict(value),

            RequestHandlerError::ResolvingRouteFailed(RouteResolverError::NoMatchingRoute) => {
                Self::not_found(value)
            }

            RequestHandlerError::OidcTokenExchangeFailed
            | RequestHandlerError::UnknownOidcState => Self::forbidden(value),

            RequestHandlerError::AgentResponseTypeMismatch { .. }
            | RequestHandlerError::InvariantViolated { .. }
            | RequestHandlerError::AgentInvocationFailed(_)
            | RequestHandlerError::InternalError(_)
            | RequestHandlerError::ResolvingRouteFailed(RouteResolverError::CouldNotBuildRouter) => {
                Self::internal(value)
            }
            RequestHandlerError::OpenApiSpecGenerationFailed { .. } => Self::internal(value),
        }
    }
}
