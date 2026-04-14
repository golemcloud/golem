// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use golem_common::base_model::api;
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
    pub code: String,
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
    pub fn unauthorized<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::Unauthorized(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn forbidden<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn internal<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::InternalError(Json(ErrorBodyWithOptionalWorkerError {
            error: error.to_safe_string(),
            worker_error: None,
            code: code.to_string(),
        }))
    }

    pub fn internal_worker(code: &str, error: String, worker_error: WorkerErrorDetails) -> Self {
        Self::InternalError(Json(ErrorBodyWithOptionalWorkerError {
            error,
            worker_error: Some(worker_error),
            code: code.to_string(),
        }))
    }

    pub fn bad_request<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::BadRequest(Json(ErrorsBody {
            errors: vec![error.to_safe_string()],
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn not_found<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::NotFound(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn conflict<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::Conflict(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn conflict_message(code: &str, error: impl ToString) -> Self {
        Self::Conflict(Json(ErrorBody {
            error: error.to_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn limit_exceeded<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::LimitExceeded(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }
}

impl From<WorkerServiceError> for ApiEndpointError {
    fn from(error: WorkerServiceError) -> Self {
        match error {
            WorkerServiceError::Internal(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, error)
            }

            WorkerServiceError::FileNotFound(_) => {
                Self::not_found(api::error_code::FILE_NOT_FOUND, error)
            }
            WorkerServiceError::TypeChecker(_) => {
                Self::bad_request(api::error_code::TYPE_CHECKER_ERROR, error)
            }
            WorkerServiceError::BadFileType(_) => {
                Self::bad_request(api::error_code::BAD_FILE_TYPE, error)
            }
            WorkerServiceError::ComponentNotFound(_) => {
                Self::not_found(api::error_code::COMPONENT_NOT_FOUND, error)
            }
            WorkerServiceError::AccountIdNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            WorkerServiceError::AgentNotFound(_) => {
                Self::not_found(api::error_code::AGENT_NOT_FOUND, error)
            }
            WorkerServiceError::GolemError(inner) => inner.into(),
            WorkerServiceError::Component(inner) => inner.into(),
            WorkerServiceError::InternalCallError(inner) => inner.into(),
            WorkerServiceError::LimitError(inner) => inner.into(),
            WorkerServiceError::AuthError(inner) => inner.into(),
            WorkerServiceError::RegistryServiceError(inner) => inner.into(),
        }
    }
}

impl From<LimitServiceError> for ApiEndpointError {
    fn from(error: LimitServiceError) -> Self {
        match error {
            LimitServiceError::LimitExceeded(_) => {
                Self::limit_exceeded(api::error_code::LIMIT_EXCEEDED, error)
            }
            LimitServiceError::InternalError(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, error)
            }
        }
    }
}

impl From<ComponentServiceError> for ApiEndpointError {
    fn from(error: ComponentServiceError) -> Self {
        match error {
            ComponentServiceError::ComponentNotFound => {
                Self::not_found(api::error_code::COMPONENT_NOT_FOUND, error)
            }
            ComponentServiceError::InternalError(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, error)
            }
        }
    }
}

impl From<CallWorkerExecutorError> for ApiEndpointError {
    fn from(error: CallWorkerExecutorError) -> Self {
        match error {
            CallWorkerExecutorError::FailedToConnectToPod(_) => {
                Self::internal(api::error_code::INTERNAL_ROUTING_FAILURE, error)
            }
            CallWorkerExecutorError::FailedToGetRoutingTable(_) => {
                Self::internal(api::error_code::INTERNAL_ROUTING_FAILURE, error)
            }
        }
    }
}

impl From<WorkerExecutorError> for ApiEndpointError {
    fn from(error: WorkerExecutorError) -> Self {
        match error {
            WorkerExecutorError::AgentNotFound { .. } => {
                Self::not_found(api::error_code::AGENT_NOT_FOUND, error)
            }
            WorkerExecutorError::InvocationFailed { error, stderr } => Self::internal_worker(
                api::error_code::INTERNAL_AGENT_EXECUTION_FAILED,
                "Invocation Failed".to_string(),
                WorkerErrorDetails {
                    cause: error.message().to_string(),
                    stderr,
                },
            ),
            WorkerExecutorError::PreviousInvocationFailed { error, stderr } => {
                Self::internal_worker(
                    api::error_code::INTERNAL_PREVIOUS_INVOCATION_FAILED,
                    "Previous Invocation Failed".to_string(),
                    WorkerErrorDetails {
                        cause: error.message().to_string(),
                        stderr,
                    },
                )
            }
            WorkerExecutorError::FailedToResumeAgent { .. } => {
                Self::internal(api::error_code::INTERNAL_AGENT_RESUME_FAILED, error)
            }
            WorkerExecutorError::ComponentDownloadFailed { .. } => {
                Self::internal(api::error_code::INTERNAL_COMPONENT_DOWNLOAD_FAILED, error)
            }
            WorkerExecutorError::GetLatestVersionOfComponentFailed { .. } => {
                Self::internal(api::error_code::INTERNAL_COMPONENT_DOWNLOAD_FAILED, error)
            }
            WorkerExecutorError::InitialAgentFileDownloadFailed { .. } => {
                Self::internal(api::error_code::INTERNAL_COMPONENT_DOWNLOAD_FAILED, error)
            }
            WorkerExecutorError::ComponentParseFailed { .. } => {
                Self::internal(api::error_code::INTERNAL_COMPONENT_PARSE_FAILED, error)
            }
            WorkerExecutorError::FileSystemError { .. } => {
                Self::internal(api::error_code::INTERNAL_FILESYSTEM_ERROR, error)
            }
            WorkerExecutorError::ShardingNotReady => {
                Self::internal(api::error_code::INTERNAL_SHARDING_NOT_READY, error)
            }
            WorkerExecutorError::NoValueInMessage => {
                Self::internal(api::error_code::INTERNAL_INVARIANT_VIOLATION, error)
            }
            WorkerExecutorError::ValueMismatch { .. } => {
                Self::internal(api::error_code::INTERNAL_INVARIANT_VIOLATION, error)
            }
            WorkerExecutorError::ParamTypeMismatch { .. } => {
                Self::internal(api::error_code::INTERNAL_INVARIANT_VIOLATION, error)
            }
            WorkerExecutorError::UnexpectedOplogEntry { .. } => {
                Self::internal(api::error_code::INTERNAL_INVARIANT_VIOLATION, error)
            }
            _ => Self::internal(api::error_code::INTERNAL_UNKNOWN, error),
        }
    }
}

impl From<AuthorizationError> for ApiEndpointError {
    fn from(value: AuthorizationError) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: value.to_string(),
            code: api::error_code::AUTH_FORBIDDEN.to_string(),
            cause: None,
        }))
    }
}

impl From<AuthServiceError> for ApiEndpointError {
    fn from(error: AuthServiceError) -> Self {
        match error {
            AuthServiceError::Unauthorized(inner) => inner.into(),
            AuthServiceError::CouldNotAuthenticate => {
                Self::unauthorized(api::error_code::AUTH_UNAUTHORIZED, error)
            }
            AuthServiceError::InternalError(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, error)
            }
        }
    }
}

impl From<RegistryServiceError> for ApiEndpointError {
    fn from(value: RegistryServiceError) -> Self {
        match value {
            RegistryServiceError::BadRequest(_) => {
                Self::bad_request(api::error_code::VALIDATION_ERROR, value)
            }
            RegistryServiceError::Unauthorized(_) => {
                Self::unauthorized(api::error_code::AUTH_UNAUTHORIZED, value)
            }
            RegistryServiceError::LimitExceeded(_) => {
                Self::limit_exceeded(api::error_code::LIMIT_EXCEEDED, value)
            }
            RegistryServiceError::NotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_NOT_FOUND, value)
            }
            RegistryServiceError::AlreadyExists(_) => {
                Self::conflict(api::error_code::RESOURCE_ALREADY_EXISTS, value)
            }
            RegistryServiceError::InternalServerError(_) => {
                Self::internal(api::error_code::INTERNAL_DEPENDENCY_FAILURE, value)
            }
            RegistryServiceError::CouldNotAuthenticate(_) => {
                Self::unauthorized(api::error_code::AUTH_UNAUTHORIZED, value)
            }
            RegistryServiceError::InternalClientError(_) => {
                Self::internal(api::error_code::INTERNAL_CLIENT_ERROR, value)
            }
        }
    }
}

impl From<RequestHandlerError> for ApiEndpointError {
    fn from(value: RequestHandlerError) -> Self {
        match value {
            RequestHandlerError::ValueParsingFailed { .. } => {
                Self::bad_request(api::error_code::REQUEST_VALUE_PARSING_FAILED, value)
            }
            RequestHandlerError::MissingValue { .. } => {
                Self::bad_request(api::error_code::REQUEST_MISSING_VALUE, value)
            }
            RequestHandlerError::TooManyValues { .. } => {
                Self::bad_request(api::error_code::REQUEST_TOO_MANY_VALUES, value)
            }
            RequestHandlerError::HeaderIsNotAscii { .. } => {
                Self::bad_request(api::error_code::REQUEST_HEADER_NOT_ASCII, value)
            }
            RequestHandlerError::BodyIsNotValidJson { .. } => {
                Self::bad_request(api::error_code::REQUEST_BODY_INVALID_JSON, value)
            }
            RequestHandlerError::JsonBodyParsingFailed { .. } => {
                Self::bad_request(api::error_code::REQUEST_JSON_BODY_PARSING_FAILED, value)
            }
            RequestHandlerError::UnsupportedMimeType { .. } => {
                Self::bad_request(api::error_code::REQUEST_UNSUPPORTED_MIME_TYPE, value)
            }
            RequestHandlerError::ResolvingRouteFailed(
                RouteResolverError::CouldNotGetDomainFromRequest(_),
            ) => Self::bad_request(api::error_code::REQUEST_DOMAIN_EXTRACTION_FAILED, value),
            RequestHandlerError::ResolvingRouteFailed(RouteResolverError::MalformedPath(_)) => {
                Self::bad_request(api::error_code::REQUEST_MALFORMED_PATH, value)
            }

            RequestHandlerError::OidcSchemeMismatch => {
                Self::conflict(api::error_code::OIDC_SCHEME_MISMATCH, value)
            }

            RequestHandlerError::ResolvingRouteFailed(RouteResolverError::NoMatchingRoute) => {
                Self::not_found(api::error_code::ROUTE_NOT_FOUND, value)
            }

            RequestHandlerError::OidcTokenExchangeFailed => {
                Self::forbidden(api::error_code::OIDC_TOKEN_EXCHANGE_FAILED, value)
            }
            RequestHandlerError::UnknownOidcState => {
                Self::forbidden(api::error_code::UNKNOWN_OIDC_STATE, value)
            }

            RequestHandlerError::AgentResponseTypeMismatch { .. } => {
                Self::internal(api::error_code::INTERNAL_INVARIANT_VIOLATION, value)
            }
            RequestHandlerError::InvariantViolated { .. } => {
                Self::internal(api::error_code::INTERNAL_INVARIANT_VIOLATION, value)
            }
            RequestHandlerError::ResolvingRouteFailed(RouteResolverError::CouldNotBuildRouter) => {
                Self::internal(api::error_code::INTERNAL_ROUTING_FAILURE, value)
            }
            RequestHandlerError::AgentInvocationFailed(_) => {
                Self::internal(api::error_code::INTERNAL_AGENT_EXECUTION_FAILED, value)
            }
            RequestHandlerError::InternalError(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, value)
            }
            RequestHandlerError::OpenApiSpecGenerationFailed => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, value)
            }
        }
    }
}
