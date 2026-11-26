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
use golem_common::model::authority::{DesignatedKind, GrantText};
use golem_common::model::error::ErrorBody;
use golem_common::model::error::ErrorsBody;
use golem_common::model::error::ForbiddenErrorBody;
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::model::auth::denial::{DenialRenderer, FORBIDDEN_MESSAGE, UnknownId};
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
    Forbidden(Json<ForbiddenErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    Conflict(Json<ErrorBody>),
    #[oai(status = 413)]
    PayloadTooLarge(Json<ErrorBody>),
    #[oai(status = 415)]
    UnsupportedMediaType(Json<ErrorBody>),
    /// A limit was exceeded, or the idempotency key was already used for a different request.
    #[oai(status = 422)]
    UnprocessableEntity(Json<ErrorBody>),
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
            Self::PayloadTooLarge(_) => "PayloadTooLarge",
            Self::UnsupportedMediaType(_) => "UnsupportedMediaType",
            Self::UnprocessableEntity(body)
                if body.code == api::error_code::IDEMPOTENCY_CONFLICT =>
            {
                "InvocationIdentityConflict"
            }
            Self::UnprocessableEntity(body)
                if body.code == api::error_code::CREATE_IDEMPOTENCY_KEY_CONFLICT =>
            {
                "CreateIdempotencyKeyConflict"
            }
            Self::UnprocessableEntity(_) => "LimitExceeded",
            Self::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            Self::BadRequest(_) => true,
            Self::NotFound(_) => true,
            Self::Conflict(_) => true,
            Self::PayloadTooLarge(_) => true,
            Self::UnsupportedMediaType(_) => true,
            Self::UnprocessableEntity(_) => true,
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
            Self::PayloadTooLarge(inner) => inner.cause.take(),
            Self::UnsupportedMediaType(inner) => inner.cause.take(),
            Self::UnprocessableEntity(inner) => inner.cause.take(),
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
        Self::Forbidden(Json(ForbiddenErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            missing_grant: None,
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

    pub fn unprocessable_entity<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::UnprocessableEntity(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn payload_too_large<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::PayloadTooLarge(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn unsupported_media_type<T: SafeDisplay>(code: &str, error: T) -> Self {
        Self::UnsupportedMediaType(Json(ErrorBody {
            error: error.to_safe_string(),
            code: code.to_string(),
            cause: None,
        }))
    }
}

/// The REST renderer of denials. The real "not found" answers of designated kinds come from
/// [`RestDenials::unknown`] too, so a denial and an unknown ID give the same bytes.
pub struct RestDenials;

impl DenialRenderer for RestDenials {
    type Response = ApiEndpointError;

    fn unknown(&self, id: &UnknownId) -> ApiEndpointError {
        ApiEndpointError::NotFound(Json(ErrorBody {
            error: id.message(),
            code: id.code().to_string(),
            cause: None,
        }))
    }

    fn forbidden(&self, missing_grant: Option<&GrantText>) -> ApiEndpointError {
        ApiEndpointError::Forbidden(Json(ForbiddenErrorBody {
            error: FORBIDDEN_MESSAGE.to_string(),
            code: api::error_code::AUTH_FORBIDDEN.to_string(),
            missing_grant: missing_grant.map(|grant| grant.0.clone()),
            cause: None,
        }))
    }

    fn sign_in_required(&self, _scheme: Option<&SecuritySchemeId>) -> ApiEndpointError {
        ApiEndpointError::Unauthorized(Json(ErrorBody {
            error: "Sign-in required".to_string(),
            code: api::error_code::AUTH_UNAUTHORIZED.to_string(),
            cause: None,
        }))
    }
}

impl From<WorkerServiceError> for ApiEndpointError {
    fn from(error: WorkerServiceError) -> Self {
        match error {
            WorkerServiceError::AcceptedInvocation { source, .. } => (*source).into(),
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
            WorkerServiceError::FileRead(
                golem_common::model::filesystem::FileReadError::InvalidTarget
                | golem_common::model::filesystem::FileReadError::InvalidSelection,
            ) => Self::bad_request(api::error_code::VALIDATION_ERROR, error),
            WorkerServiceError::FileRead(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, error)
            }
            WorkerServiceError::ComponentNotFound(_) => {
                Self::not_found(api::error_code::COMPONENT_NOT_FOUND, error)
            }
            WorkerServiceError::AccountIdNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            // A denied agent answers as an unknown one (5.3 Rule 21).
            WorkerServiceError::AgentNotFound(agent_id)
            | WorkerServiceError::AgentAccessDenied(agent_id) => {
                RestDenials.unknown(&UnknownId::new(DesignatedKind::Agent, agent_id))
            }
            WorkerServiceError::InvocationNotFound(invocation_id) => {
                RestDenials.unknown(&UnknownId::new(DesignatedKind::Invocation, invocation_id))
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
                Self::unprocessable_entity(api::error_code::LIMIT_EXCEEDED, error)
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
            CallWorkerExecutorError::MessageTooLarge(_) => {
                Self::payload_too_large(api::error_code::REQUEST_PAYLOAD_TOO_LARGE, error)
            }
        }
    }
}

impl From<WorkerExecutorError> for ApiEndpointError {
    fn from(error: WorkerExecutorError) -> Self {
        match error {
            WorkerExecutorError::InvalidRequest { ref details }
                if details.starts_with("IdempotencyConflict:") =>
            {
                Self::UnprocessableEntity(Json(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::IDEMPOTENCY_CONFLICT.to_string(),
                    cause: None,
                }))
            }
            WorkerExecutorError::InvalidRequest { .. } => {
                Self::bad_request(api::error_code::VALIDATION_ERROR, error)
            }
            WorkerExecutorError::StaleCursor { .. } => {
                Self::bad_request(api::error_code::STALE_CURSOR, error)
            }
            WorkerExecutorError::AgentNotFound { agent_id } => {
                RestDenials.unknown(&UnknownId::new(DesignatedKind::Agent, agent_id))
            }
            WorkerExecutorError::PromiseNotFound { promise_id } => {
                RestDenials.unknown(&UnknownId::new(DesignatedKind::Promise, promise_id))
            }
            WorkerExecutorError::AgentAlreadyExists { .. } => {
                Self::conflict(api::error_code::AGENT_ALREADY_EXISTS, error)
            }
            WorkerExecutorError::AgentKeyCollision { .. } => {
                Self::conflict(api::error_code::AGENT_KEY_COLLISION, error)
            }
            WorkerExecutorError::CreateIdempotencyKeyConflict { .. } => {
                Self::unprocessable_entity(api::error_code::CREATE_IDEMPOTENCY_KEY_CONFLICT, error)
            }
            WorkerExecutorError::InvocationCancelled => {
                Self::conflict(api::error_code::INVOCATION_CANCELLED, error)
            }
            // Until running invocations can be cancelled, this is a conflict instead of `202`.
            WorkerExecutorError::CannotCancelRunningInvocation => {
                Self::conflict(api::error_code::CANNOT_CANCEL_RUNNING_INVOCATION, error)
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
            WorkerExecutorError::ComponentDownloadFailed { .. }
            | WorkerExecutorError::ComponentServiceUnavailable { .. }
            | WorkerExecutorError::ComponentServiceRefused { .. } => {
                Self::internal(api::error_code::INTERNAL_COMPONENT_DOWNLOAD_FAILED, error)
            }
            WorkerExecutorError::GetCurrentVersionOfComponentFailed { .. } => {
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
            WorkerExecutorError::ComponentNotFound { .. } => {
                Self::not_found(api::error_code::COMPONENT_NOT_FOUND, error)
            }
            _ => Self::internal(api::error_code::INTERNAL_UNKNOWN, error),
        }
    }
}

impl From<AuthorizationError> for ApiEndpointError {
    /// A denied permission is the renderer's `403`, so the two cannot drift apart. The other
    /// variants keep their own texts, which name an action and no account.
    fn from(value: AuthorizationError) -> Self {
        match value {
            AuthorizationError::PermissionNotAllowed(_)
            | AuthorizationError::PermissionEvaluationFailed { .. } => RestDenials.forbidden(None),
            other => Self::forbidden(api::error_code::AUTH_FORBIDDEN, other),
        }
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
                Self::unprocessable_entity(api::error_code::LIMIT_EXCEEDED, value)
            }
            RegistryServiceError::NotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_NOT_FOUND, value)
            }
            RegistryServiceError::AlreadyExists(_) => {
                Self::conflict(api::error_code::RESOURCE_ALREADY_EXISTS, value)
            }
            RegistryServiceError::RuntimeCardRevoked(_) => {
                Self::conflict(api::error_code::RUNTIME_CARD_REVOKED, value)
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
            RequestHandlerError::ParameterParsingFailed { ref error, .. } => {
                let code = match error.as_ref() {
                    RequestHandlerError::MissingValue { .. } => {
                        api::error_code::REQUEST_MISSING_VALUE
                    }
                    RequestHandlerError::TooManyValues { .. } => {
                        api::error_code::REQUEST_TOO_MANY_VALUES
                    }
                    _ => api::error_code::REQUEST_VALUE_PARSING_FAILED,
                };
                Self::bad_request(code, value)
            }
            RequestHandlerError::ValueParsingFailed { .. }
            | RequestHandlerError::InvalidParameter { .. }
            | RequestHandlerError::UnknownQueryParameter { .. } => {
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
            RequestHandlerError::BodyIsNotValidUtf8 { .. } => {
                Self::bad_request(api::error_code::REQUEST_BODY_INVALID_UTF8, value)
            }
            RequestHandlerError::UnsupportedLanguage { .. } => {
                Self::unsupported_media_type(api::error_code::REQUEST_UNSUPPORTED_LANGUAGE, value)
            }
            RequestHandlerError::UnsupportedTextContentType { .. } => Self::unsupported_media_type(
                api::error_code::REQUEST_UNSUPPORTED_TEXT_CONTENT_TYPE,
                value,
            ),
            RequestHandlerError::MultiValuedContentLanguageHeader => Self::bad_request(
                api::error_code::REQUEST_MULTI_VALUED_CONTENT_LANGUAGE,
                value,
            ),
            RequestHandlerError::ResolvingRouteFailed(
                RouteResolverError::CouldNotGetDomainFromRequest(_),
            ) => Self::bad_request(api::error_code::REQUEST_DOMAIN_EXTRACTION_FAILED, value),
            RequestHandlerError::ResolvingRouteFailed(RouteResolverError::MalformedPath(_)) => {
                Self::bad_request(api::error_code::REQUEST_MALFORMED_PATH, value)
            }

            RequestHandlerError::OidcSchemeMismatch => {
                Self::conflict(api::error_code::OIDC_SCHEME_MISMATCH, value)
            }

            RequestHandlerError::ResolvingRouteFailed(
                RouteResolverError::NoMatchingRoute | RouteResolverError::UnknownSite,
            ) => Self::not_found(api::error_code::ROUTE_NOT_FOUND, value),

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
            RequestHandlerError::AgentInvocationFailed(
                WorkerServiceError::AcceptedInvocation { source, .. },
            ) => RequestHandlerError::AgentInvocationFailed(*source).into(),
            RequestHandlerError::AgentInvocationFailed(WorkerServiceError::InternalCallError(
                CallWorkerExecutorError::MessageTooLarge(_),
            )) => Self::payload_too_large(api::error_code::REQUEST_PAYLOAD_TOO_LARGE, value),
            RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
                error @ WorkerExecutorError::InvalidRequest { .. },
            )) if matches!(&error, WorkerExecutorError::InvalidRequest { details } if details.starts_with("IdempotencyConflict:")) => {
                error.into()
            }
            RequestHandlerError::AgentInvocationFailed(WorkerServiceError::GolemError(
                error @ WorkerExecutorError::AgentKeyCollision { .. },
            )) => error.into(),
            RequestHandlerError::AgentInvocationFailed(
                WorkerServiceError::AgentNotFound(_)
                | WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { .. }),
            ) => Self::not_found(api::error_code::AGENT_NOT_FOUND, value),
            RequestHandlerError::AgentInvocationFailed(_) => {
                Self::internal(api::error_code::INTERNAL_AGENT_EXECUTION_FAILED, value)
            }
            RequestHandlerError::InternalError(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, value)
            }
            RequestHandlerError::OpenApi(_) => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, value)
            }
            RequestHandlerError::RawRequest(_) => {
                Self::bad_request(api::error_code::REQUEST_VALUE_PARSING_FAILED, value)
            }
            RequestHandlerError::RawMessageTooLarge(_) => {
                Self::payload_too_large(api::error_code::REQUEST_PAYLOAD_TOO_LARGE, value)
            }
            RequestHandlerError::RawBadGateway
            | RequestHandlerError::RawDeadline
            | RequestHandlerError::RawAcceptanceTimeout => {
                Self::internal(api::error_code::INTERNAL_AGENT_EXECUTION_FAILED, value)
            }
            RequestHandlerError::RawInternal => {
                Self::internal(api::error_code::INTERNAL_UNKNOWN, value)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::worker::{CallWorkerExecutorError, WorkerServiceError};
    use poem::IntoResponse;
    use test_r::test;
    use tonic::{Code, Status};

    #[test]
    async fn create_conflicts_preserve_their_codes_through_grpc_and_http() {
        use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
        use golem_common::model::AgentId;
        use poem::IntoResponse;
        use poem::http::StatusCode;
        use prost::Message;

        let agent_id = AgentId::from_parts(7, 1);
        for (error, status, code) in [
            (
                WorkerExecutorError::AgentAlreadyExists { agent_id },
                StatusCode::CONFLICT,
                api::error_code::AGENT_ALREADY_EXISTS,
            ),
            (
                WorkerExecutorError::AgentKeyCollision {
                    agent_id,
                    requested_key: "Counter(\"a\")".to_string(),
                },
                StatusCode::CONFLICT,
                api::error_code::AGENT_KEY_COLLISION,
            ),
            (
                WorkerExecutorError::CreateIdempotencyKeyConflict { agent_id },
                StatusCode::UNPROCESSABLE_ENTITY,
                api::error_code::CREATE_IDEMPOTENCY_KEY_CONFLICT,
            ),
        ] {
            let encoded = WorkerExecutionError::from(error).encode_to_vec();
            let transported = WorkerExecutorError::try_from(
                WorkerExecutionError::decode(encoded.as_slice()).unwrap(),
            )
            .expect("the executor error survives the create response");
            let api_error = ApiEndpointError::from(transported);
            if code == api::error_code::CREATE_IDEMPOTENCY_KEY_CONFLICT {
                assert_eq!(api_error.trace_error_kind(), "CreateIdempotencyKeyConflict");
            }
            let response = api_error.into_response();
            assert_eq!(response.status(), status);
            let body = response
                .into_body()
                .into_json::<serde_json::Value>()
                .await
                .unwrap();
            assert_eq!(body["code"], code);
        }
        let limit: ApiEndpointError = LimitServiceError::LimitExceeded("quota".into()).into();
        let response = limit.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = response
            .into_body()
            .into_json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(body["code"], api::error_code::LIMIT_EXCEEDED);
    }

    #[test]
    async fn invocation_identity_conflict_maps_to_422_at_public_error_boundaries() {
        for details in [
            "IdempotencyConflict: invocation identity was already accepted with a different request",
            "IdempotencyConflict: runtime card changed",
        ] {
            for custom_api in [false, true] {
                let error =
                    WorkerServiceError::GolemError(WorkerExecutorError::invalid_request(details));
                let api_error = if custom_api {
                    ApiEndpointError::from(RequestHandlerError::AgentInvocationFailed(error))
                } else {
                    ApiEndpointError::from(error)
                };
                assert!(api_error.is_expected());
                assert_eq!(api_error.trace_error_kind(), "InvocationIdentityConflict");
                let response = api_error.into_response();
                assert_eq!(response.status(), http::StatusCode::UNPROCESSABLE_ENTITY);
                let body: serde_json::Value = response.into_body().into_json().await.unwrap();
                assert_eq!(body["code"], api::error_code::IDEMPOTENCY_CONFLICT);
                assert_eq!(body["error"], format!("Invalid request: {details}"));
            }
        }
    }

    #[test]
    fn unrelated_invalid_requests_keep_public_error_statuses() {
        for details in [
            "invalid scan cursor",
            "StaleEpoch: attachment is stale",
            "InvalidAttachmentState: attachment is active",
            "InputConflict: input frame changed",
            "invalid IdempotencyConflict: not an identity conflict prefix",
        ] {
            let error =
                WorkerServiceError::GolemError(WorkerExecutorError::invalid_request(details));
            let response = ApiEndpointError::from(error).into_response();
            assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);

            let error =
                WorkerServiceError::GolemError(WorkerExecutorError::invalid_request(details));
            let response =
                ApiEndpointError::from(RequestHandlerError::AgentInvocationFailed(error))
                    .into_response();
            assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    fn message_too_large_status() -> Status {
        Status::new(
            Code::ResourceExhausted,
            "Error decompressing: size limit, of 4194304 bytes, exceeded while decompressing message",
        )
    }

    #[test]
    fn message_size_limit_status_maps_to_payload_too_large() {
        let error = CallWorkerExecutorError::MessageTooLarge(message_too_large_status());

        let api_error: ApiEndpointError = error.into();
        match api_error {
            ApiEndpointError::PayloadTooLarge(Json(body)) => {
                assert_eq!(body.code, api::error_code::REQUEST_PAYLOAD_TOO_LARGE);
                assert!(body.error.contains(message_too_large_status().message()));
            }
            other => panic!("expected PayloadTooLarge, got: {other:?}"),
        }
    }

    #[test]
    fn custom_api_message_too_large_maps_to_payload_too_large() {
        let inner = WorkerServiceError::InternalCallError(
            CallWorkerExecutorError::MessageTooLarge(message_too_large_status()),
        );
        let error = RequestHandlerError::AgentInvocationFailed(inner);

        let api_error: ApiEndpointError = error.into();
        match api_error {
            ApiEndpointError::PayloadTooLarge(Json(body)) => {
                assert_eq!(body.code, api::error_code::REQUEST_PAYLOAD_TOO_LARGE);
                assert!(body.error.contains(message_too_large_status().message()));
            }
            other => panic!("expected PayloadTooLarge, got: {other:?}"),
        }
    }

    #[test]
    fn invalid_worker_executor_request_maps_to_validation_error() {
        let api_error: ApiEndpointError =
            WorkerExecutorError::invalid_request("invalid scan cursor").into();

        match api_error {
            ApiEndpointError::BadRequest(Json(body)) => {
                assert_eq!(body.code, api::error_code::VALIDATION_ERROR);
                assert_eq!(body.errors, vec!["Invalid request: invalid scan cursor"]);
            }
            other => panic!("expected BadRequest, got: {other:?}"),
        }
    }

    #[test]
    fn stale_oplog_cursor_maps_to_its_own_code() {
        use golem_service_base::error::worker_executor::StaleCursorReason;

        for reason in [
            StaleCursorReason::PastEnd {
                last_oplog_index: golem_common::model::OplogIndex::from_u64(17),
            },
            StaleCursorReason::QueryMismatch,
        ] {
            let api_error: ApiEndpointError =
                WorkerServiceError::GolemError(WorkerExecutorError::StaleCursor {
                    agent_id: golem_common::model::AgentId::from_parts(7, 3),
                    next_oplog_index: golem_common::model::OplogIndex::from_u64(42),
                    reason,
                })
                .into();

            match api_error {
                ApiEndpointError::BadRequest(Json(body)) => {
                    assert_eq!(body.code, "STALE_CURSOR");
                }
                other => panic!("expected BadRequest, got: {other:?}"),
            }
        }
    }

    #[test]
    fn internal_worker_executor_error_still_maps_to_internal_error() {
        let api_error: ApiEndpointError = WorkerExecutorError::ShardingNotReady.into();

        match api_error {
            ApiEndpointError::InternalError(Json(body)) => {
                assert_eq!(body.code, api::error_code::INTERNAL_SHARDING_NOT_READY);
            }
            other => panic!("expected InternalError, got: {other:?}"),
        }
    }

    #[test]
    fn cancelled_and_running_cancel_map_to_distinct_conflicts() {
        for (error, code) in [
            (
                WorkerExecutorError::InvocationCancelled,
                api::error_code::INVOCATION_CANCELLED,
            ),
            (
                WorkerExecutorError::CannotCancelRunningInvocation,
                api::error_code::CANNOT_CANCEL_RUNNING_INVOCATION,
            ),
        ] {
            match ApiEndpointError::from(error) {
                ApiEndpointError::Conflict(Json(body)) => assert_eq!(body.code, code),
                other => panic!("expected Conflict, got: {other:?}"),
            }
        }
    }

    #[test]
    async fn acceptance_metadata_preserves_public_error_responses() {
        use crate::service::worker::AcceptedInvocation;
        use golem_common::model::environment::EnvironmentId;
        use golem_common::model::{AgentId, InvocationId, OwnedInvocationId};
        use poem::IntoResponse;

        let wrap = |source| WorkerServiceError::AcceptedInvocation {
            accepted: AcceptedInvocation {
                id: OwnedInvocationId::new(
                    AgentId::from_parts(7, 1),
                    InvocationId(uuid::Uuid::new_v4()),
                ),
                environment_id: EnvironmentId::new(),
            },
            source: Box::new(source),
        };
        for make_error in [
            (|| {
                WorkerServiceError::GolemError(WorkerExecutorError::invalid_request(
                    "IdempotencyConflict: different input",
                ))
            }) as fn() -> WorkerServiceError,
            || WorkerServiceError::GolemError(WorkerExecutorError::InvocationCancelled),
            || {
                WorkerServiceError::RegistryServiceError(RegistryServiceError::LimitExceeded(
                    "limit reached".into(),
                ))
            },
            || WorkerServiceError::TypeChecker("invalid input".into()),
        ] {
            let plain: golem_api_grpc::proto::golem::worker::v1::AgentError = make_error().into();
            let decorated: golem_api_grpc::proto::golem::worker::v1::AgentError =
                wrap(make_error()).into();
            assert_eq!(plain, decorated);
            for custom_api in [false, true] {
                let response = |error| {
                    if custom_api {
                        ApiEndpointError::from(RequestHandlerError::AgentInvocationFailed(error))
                    } else {
                        ApiEndpointError::from(error)
                    }
                    .into_response()
                };
                let plain = response(make_error());
                let decorated = response(wrap(make_error()));
                assert_eq!(plain.status(), decorated.status());
                assert_eq!(
                    plain.into_body().into_vec().await.unwrap(),
                    decorated.into_body().into_vec().await.unwrap()
                );
            }
        }
    }

    /// The status, the headers and the body bytes that a caller receives for `error`.
    async fn wire(
        error: ApiEndpointError,
    ) -> (poem::http::StatusCode, Vec<(String, Vec<u8>)>, Vec<u8>) {
        let response = error.into_response();
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
            .collect();
        (
            status,
            headers,
            response.into_body().into_vec().await.unwrap(),
        )
    }

    fn json(body: &[u8]) -> serde_json::Value {
        serde_json::from_slice(body).unwrap()
    }

    /// A 1.6 permission target whose text holds the owner's account.
    fn target_of_another_account() -> Box<golem_common::model::card::PermissionTarget> {
        use golem_common::model::account::AccountEmail;
        use golem_common::model::card::owner::AgentOwnerPattern;
        use golem_common::model::card::{
            AgentResourcePattern, AgentVerb, ClassPermissionTarget, PermissionTarget,
        };

        Box::new(PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::AccountAgents {
                account: AccountEmail::new("owner@example.com"),
            },
            verb: Some(AgentVerb::Invoke),
            resource: AgentResourcePattern::Any,
        }))
    }

    #[test]
    async fn a_denied_agent_is_byte_identical_to_an_unknown_agent() {
        use golem_common::model::AgentId;

        let agent_id = AgentId::from_parts(7, 3);
        let unknown = wire(WorkerServiceError::AgentNotFound(agent_id).into()).await;
        assert_eq!(unknown.0, poem::http::StatusCode::NOT_FOUND);
        assert_eq!(
            json(&unknown.2),
            serde_json::json!({
                "code": api::error_code::AGENT_NOT_FOUND,
                "error": format!("Worker not found: {agent_id}"),
            })
        );

        let denied = wire(WorkerServiceError::AgentAccessDenied(agent_id).into()).await;
        assert_eq!(
            denied, unknown,
            "a denial must not differ from an unknown ID"
        );

        let unknown_in_the_executor = wire(
            WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { agent_id }).into(),
        )
        .await;
        assert_eq!(unknown_in_the_executor, unknown);
    }

    #[test]
    async fn an_unknown_promise_is_a_404_with_its_own_code() {
        use golem_common::model::{AgentFingerprint, AgentId, OplogIndex, PromiseId};

        let promise_id = PromiseId {
            agent_id: AgentId::from_parts(7, 3),
            fingerprint: AgentFingerprint::new(),
            oplog_idx: OplogIndex::from_u64(5),
        };
        let (status, _, body) = wire(
            WorkerServiceError::GolemError(WorkerExecutorError::PromiseNotFound {
                promise_id: promise_id.clone(),
            })
            .into(),
        )
        .await;
        assert_eq!(status, poem::http::StatusCode::NOT_FOUND);
        assert_eq!(
            json(&body),
            serde_json::json!({
                "code": api::error_code::PROMISE_NOT_FOUND,
                "error": format!("Promise not found: {promise_id}"),
            })
        );
    }

    #[test]
    async fn every_designated_kind_renders_as_a_404_with_its_own_code() {
        use golem_common::model::authority::{Denial, DesignatedKind};
        use golem_service_base::model::auth::denial::{DenialRenderer, UnknownId};

        for kind in [
            DesignatedKind::Agent,
            DesignatedKind::Invocation,
            DesignatedKind::Task,
            DesignatedKind::Stream,
            DesignatedKind::Promise,
            DesignatedKind::Webhook,
            DesignatedKind::Artifact,
        ] {
            let id = UnknownId::new(kind, "id-7");
            let (status, _, body) =
                wire(RestDenials.render(&Denial::LooksUnknown { kind }, std::slice::from_ref(&id)))
                    .await;
            assert_eq!(status, poem::http::StatusCode::NOT_FOUND, "{kind:?}");
            assert_eq!(
                json(&body),
                serde_json::json!({"code": id.code(), "error": id.message()}),
                "{kind:?}"
            );
        }
    }

    #[test]
    async fn a_denied_permission_gets_the_bytes_of_the_renderers_403() {
        use golem_common::model::card::CardAlgebraError;

        let rendered = wire(RestDenials.forbidden(None)).await;
        assert_eq!(rendered.0, poem::http::StatusCode::FORBIDDEN);
        assert_eq!(
            json(&rendered.2),
            serde_json::json!({"code": api::error_code::AUTH_FORBIDDEN, "error": FORBIDDEN_MESSAGE})
        );
        for error in [
            AuthorizationError::PermissionNotAllowed(target_of_another_account()),
            AuthorizationError::PermissionEvaluationFailed {
                target: target_of_another_account(),
                error: CardAlgebraError::InvalidOwnerPath("owner@example.com/app".to_string()),
            },
        ] {
            assert_eq!(wire(error.into()).await, rendered);
        }
    }

    #[test]
    async fn forbidden_names_the_missing_grant_only_when_the_authorizer_gives_one() {
        use golem_common::model::authority::{Denial, GrantText};
        use golem_service_base::model::auth::denial::{DenialRenderer, FORBIDDEN_MESSAGE};

        let (status, _, body) = wire(RestDenials.render(
            &Denial::Forbidden {
                missing_grant: None,
            },
            &[],
        ))
        .await;
        assert_eq!(status, poem::http::StatusCode::FORBIDDEN);
        assert_eq!(
            json(&body),
            serde_json::json!({"code": api::error_code::AUTH_FORBIDDEN, "error": FORBIDDEN_MESSAGE})
        );

        let grant = GrantText("tool(E/grep) : invoke".to_string());
        let (status, _, body) = wire(RestDenials.render(
            &Denial::Forbidden {
                missing_grant: Some(grant),
            },
            &[],
        ))
        .await;
        assert_eq!(status, poem::http::StatusCode::FORBIDDEN);
        assert_eq!(
            json(&body),
            serde_json::json!({
                "code": api::error_code::AUTH_FORBIDDEN,
                "error": FORBIDDEN_MESSAGE,
                "missingGrant": "tool(E/grep) : invoke",
            })
        );
    }

    #[test]
    async fn a_denied_permission_is_a_403_that_shows_no_target() {
        use golem_service_base::model::auth::denial::FORBIDDEN_MESSAGE;

        for error in [
            ApiEndpointError::from(AuthorizationError::PermissionNotAllowed(
                target_of_another_account(),
            )),
            ApiEndpointError::from(WorkerServiceError::AuthError(
                AuthServiceError::Unauthorized(AuthorizationError::PermissionNotAllowed(
                    target_of_another_account(),
                )),
            )),
        ] {
            let (status, _, body) = wire(error).await;
            assert_eq!(status, poem::http::StatusCode::FORBIDDEN);
            assert_eq!(
                json(&body),
                serde_json::json!({
                    "code": api::error_code::AUTH_FORBIDDEN,
                    "error": FORBIDDEN_MESSAGE,
                })
            );
        }
    }
}
