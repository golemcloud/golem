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

// use crate::gateway_security::IdentityProviderError;
// use crate::service::api_certificate::CertificateServiceError;
// use crate::service::api_domain::ApiDomainServiceError;
// use crate::service::api_domain::RegisterDomainRouteError;
// use crate::service::api_security::SecuritySchemeServiceError;
use crate::service::component::ComponentServiceError;
// use crate::service::gateway::api_definition::ApiDefinitionError as BaseApiDefinitionError;
// use crate::service::gateway::api_deployment::ApiDeploymentError;
// use crate::service::gateway::security_scheme::SecuritySchemeServiceError as BaseSecuritySchemeServiceError;
use crate::service::worker::{CallWorkerExecutorError, WorkerServiceError};
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::error::ErrorBody;
use golem_common::model::error::ErrorsBody;
use golem_common::SafeDisplay;
// use golem_service_base::clients::project::ProjectError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthorizationError;
use poem_openapi::payload::Json;
use poem_openapi::ApiResponse;
use serde::{Deserialize, Serialize};
use crate::service::limit::LimitServiceError;
use crate::service::auth::AuthServiceError;

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
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBodyWithOptionalWorkerError>),
}

impl ApiErrorDetails for ApiEndpointError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            Self::BadRequest(_) => "BadRequest",
            Self::NotFound(_) => "NotFound",
            Self::AlreadyExists(_) => "AlreadyExists",
            Self::LimitExceeded(_) => "LimitExceeded",
            Self::Forbidden(_) => "Forbidden",
            Self::Unauthorized(_) => "Unauthorized",
            Self::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            Self::BadRequest(_) => true,
            Self::NotFound(_) => true,
            Self::AlreadyExists(_) => true,
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
            Self::AlreadyExists(inner) => inner.cause.take(),
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

    pub fn already_exists<T: SafeDisplay>(error: T) -> Self {
        Self::AlreadyExists(Json(ErrorBody {
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

// impl From<ApiDeploymentError> for ApiEndpointError {
//     fn from(value: ApiDeploymentError) -> Self {
//         match value {
//             ApiDeploymentError::ApiDefinitionNotFound(_, _, _) => {
//                 ApiEndpointError::not_found(value)
//             }
//             ApiDeploymentError::ApiDeploymentNotFound(_, _) => ApiEndpointError::not_found(value),
//             ApiDeploymentError::ApiDeploymentConflict(_) => ApiEndpointError::already_exists(value),
//             ApiDeploymentError::ApiDefinitionsConflict(_) => ApiEndpointError::bad_request(value),
//             ApiDeploymentError::InternalRepoError(_) => ApiEndpointError::internal(value),
//             ApiDeploymentError::InternalConversionError { .. } => ApiEndpointError::internal(value),
//             ApiDeploymentError::ComponentConstraintCreateError(_) => {
//                 ApiEndpointError::bad_request(value)
//             }
//         }
//     }
// }

// impl From<BaseSecuritySchemeServiceError> for ApiEndpointError {
//     fn from(value: BaseSecuritySchemeServiceError) -> Self {
//         match value {
//             BaseSecuritySchemeServiceError::IdentityProviderError(identity_provider_error) => {
//                 ApiEndpointError::from(identity_provider_error)
//             }
//             BaseSecuritySchemeServiceError::InternalError(_) => ApiEndpointError::internal(value),
//             BaseSecuritySchemeServiceError::NotFound(_) => ApiEndpointError::not_found(value),
//         }
//     }
// }

// impl From<SecuritySchemeServiceError> for ApiEndpointError {
//     fn from(value: SecuritySchemeServiceError) -> Self {
//         match value {
//             SecuritySchemeServiceError::Auth(error) => ApiEndpointError::from(error),
//             SecuritySchemeServiceError::Base(error) => ApiEndpointError::from(error),
//         }
//     }
// }

// impl From<IdentityProviderError> for ApiEndpointError {
//     fn from(value: IdentityProviderError) -> Self {
//         match value {
//             IdentityProviderError::ClientInitError(error) => {
//                 ApiEndpointError::internal(safe(error))
//             }
//             IdentityProviderError::InvalidIssuerUrl(error) => {
//                 ApiEndpointError::bad_request(safe(error))
//             }
//             IdentityProviderError::FailedToDiscoverProviderMetadata(error) => {
//                 ApiEndpointError::bad_request(safe(error))
//             }
//             IdentityProviderError::FailedToExchangeCodeForTokens(error) => {
//                 ApiEndpointError::unauthorized(safe(error))
//             }
//             IdentityProviderError::IdTokenVerificationError(error) => {
//                 ApiEndpointError::unauthorized(safe(error))
//             }
//         }
//     }
// }

// impl From<BaseApiDefinitionError> for ApiEndpointError {
//     fn from(value: BaseApiDefinitionError) -> Self {
//         match value {
//             BaseApiDefinitionError::ValidationError(error) => {
//                 let errors = error.errors.into_iter().collect::<Vec<_>>();

//                 let error = ErrorsBody { errors };

//                 ApiEndpointError::BadRequest(Json(error))
//             }
//             BaseApiDefinitionError::ApiDefinitionNotDraft(_) => {
//                 ApiEndpointError::bad_request(value)
//             }
//             BaseApiDefinitionError::ApiDefinitionNotFound(_) => ApiEndpointError::not_found(value),
//             BaseApiDefinitionError::ApiDefinitionAlreadyExists(_, _) => {
//                 ApiEndpointError::already_exists(value)
//             }
//             BaseApiDefinitionError::ComponentNotFoundError(_) => {
//                 ApiEndpointError::bad_request(value)
//             }
//             BaseApiDefinitionError::ApiDefinitionDeployed(_) => {
//                 ApiEndpointError::bad_request(value)
//             }
//             BaseApiDefinitionError::RibCompilationErrors(_) => ApiEndpointError::bad_request(value),
//             BaseApiDefinitionError::InternalRepoError(_) => ApiEndpointError::internal(value),
//             BaseApiDefinitionError::Internal(_) => ApiEndpointError::internal(value),
//             BaseApiDefinitionError::SecuritySchemeError(error) => ApiEndpointError::from(error),
//             BaseApiDefinitionError::IdentityProviderError(error) => ApiEndpointError::from(error),
//             BaseApiDefinitionError::RibInternal(_) => ApiEndpointError::internal(value),
//             BaseApiDefinitionError::InvalidOasDefinition(_) => ApiEndpointError::bad_request(value),
//             BaseApiDefinitionError::UnsupportedRibInput(_) => ApiEndpointError::bad_request(value),
//             BaseApiDefinitionError::RibStaticAnalysisError(_) => {
//                 ApiEndpointError::bad_request(value)
//             }
//             BaseApiDefinitionError::RibByteCodeGenerationError(_) => {
//                 ApiEndpointError::internal(value)
//             }
//             BaseApiDefinitionError::RibParseError(_) => ApiEndpointError::bad_request(value),
//         }
//     }
// }

// impl From<ProjectError> for ApiEndpointError {
//     fn from(value: ProjectError) -> Self {
//         match value {
//             ProjectError::Server(error) => match &error.error {
//                 None => ApiEndpointError::internal(safe("Unknown project error".to_string())),
//                 Some(Error::BadRequest(errors)) => ApiEndpointError::BadRequest(Json(ErrorsBody {
//                     errors: errors.errors.clone(),
//                 })),
//                 Some(Error::InternalError(error)) => {
//                     ApiEndpointError::internal(safe(error.error.to_string()))
//                 }
//                 Some(Error::NotFound(error)) => ApiEndpointError::NotFound(Json(ErrorBody {
//                     error: error.error.clone(),
//                 })),
//                 Some(Error::Unauthorized(error)) => {
//                     ApiEndpointError::Unauthorized(Json(ErrorBody {
//                         error: error.error.clone(),
//                     }))
//                 }
//                 Some(Error::LimitExceeded(error)) => {
//                     ApiEndpointError::LimitExceeded(Json(ErrorBody {
//                         error: error.error.clone(),
//                     }))
//                 }
//             },
//             ProjectError::Connection(status) => ApiEndpointError::internal(safe(format!(
//                 "Project service connection error: {status}"
//             ))),
//             ProjectError::Transport(error) => ApiEndpointError::internal(safe(format!(
//                 "Project service transport error: {error}"
//             ))),
//             ProjectError::Unknown(_) => {
//                 ApiEndpointError::internal(safe("Unknown project error".to_string()))
//             }
//         }
//     }
// }

// impl From<RegisterDomainRouteError> for ApiEndpointError {
//     fn from(error: RegisterDomainRouteError) -> Self {
//         match error {
//             RegisterDomainRouteError::NotAvailable(_) => Self::bad_request(error),
//             RegisterDomainRouteError::AWSError { .. } => Self::internal(error),
//         }
//     }
// }

// impl From<ApiDomainServiceError> for ApiEndpointError {
//     fn from(error: ApiDomainServiceError) -> Self {
//         match error {
//             ApiDomainServiceError::Unauthorized(_) => Self::unauthorized(error),
//             ApiDomainServiceError::NotFound(_) => ApiEndpointError::not_found(error),
//             ApiDomainServiceError::AlreadyExists(_) => ApiEndpointError::already_exists(error),
//             ApiDomainServiceError::InternalConversionError(_)
//             | ApiDomainServiceError::InternalRepoError(_)
//             | ApiDomainServiceError::InternalAuthClientError(_)
//             | ApiDomainServiceError::InternalAWSError { .. } => Self::internal(error),
//         }
//     }
// }

// impl From<CertificateServiceError> for ApiEndpointError {
//     fn from(error: CertificateServiceError) -> Self {
//         match error {
//             CertificateServiceError::CertificateNotAvailable(_) => Self::bad_request(error),
//             CertificateServiceError::CertificateNotFound(_) => Self::not_found(error),
//             CertificateServiceError::Unauthorized(_) => Self::unauthorized(error),

//             CertificateServiceError::InternalCertificateManagerError(_)
//             | CertificateServiceError::InternalAuthClientError(_)
//             | CertificateServiceError::InternalRepoError(_)
//             | CertificateServiceError::InternalConversionError(_) => Self::internal(error),
//         }
//     }
// }

impl From<AuthServiceError> for ApiEndpointError {
    fn from(error: AuthServiceError) -> Self {
        match error {
            AuthServiceError::CouldNotAuthenticate => Self::unauthorized(error),
            AuthServiceError::InternalError(_) => Self::internal(error),
        }
    }
}
