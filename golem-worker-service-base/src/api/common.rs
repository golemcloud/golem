// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::{Debug, Formatter};

use golem_api_grpc::proto::golem::apidefinition::v1::{api_definition_error, ApiDefinitionError};
use golem_api_grpc::proto::golem::worker;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::SafeDisplay;
use golem_service_base::model::ErrorBody;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Object, Union};

#[derive(Union, Clone, Debug)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum WorkerServiceErrorsBody {
    Messages(MessagesErrorsBody),
    Validation(ValidationErrorsBody),
}

// TODO: These should probably use golem_common ErrorBody and ErrorsBody instead.

#[derive(Clone, Debug, Object)]
pub struct MessagesErrorsBody {
    errors: Vec<String>,
}

#[derive(Clone, Debug, Object)]
pub struct ValidationErrorsBody {
    errors: Vec<String>,
}

#[derive(ApiResponse, Clone, Debug)]
pub enum ApiEndpointError {
    #[oai(status = 400)]
    BadRequest(Json<WorkerServiceErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<String>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for ApiEndpointError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ApiEndpointError::BadRequest(_) => "BadRequest",
            ApiEndpointError::NotFound(_) => "NotFound",
            ApiEndpointError::AlreadyExists(_) => "AlreadyExists",
            ApiEndpointError::Forbidden(_) => "Forbidden",
            ApiEndpointError::Unauthorized(_) => "Unauthorized",
            ApiEndpointError::InternalError(_) => "InternalError",
        }
    }
}

impl ApiEndpointError {
    pub fn unauthorized<T: SafeDisplay>(error: T) -> Self {
        Self::Unauthorized(Json(ErrorBody {
            error: error.to_safe_string(),
        }))
    }

    pub fn forbidden<T: SafeDisplay>(error: T) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: error.to_safe_string(),
        }))
    }

    pub fn internal<T: SafeDisplay>(error: T) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: error.to_safe_string(),
        }))
    }

    pub fn bad_request<T: SafeDisplay>(error: T) -> Self {
        Self::BadRequest(Json(WorkerServiceErrorsBody::Messages(
            MessagesErrorsBody {
                errors: vec![error.to_safe_string()],
            },
        )))
    }

    pub fn not_found<T: SafeDisplay>(error: T) -> Self {
        Self::NotFound(Json(ErrorBody {
            error: error.to_safe_string(),
        }))
    }

    pub fn already_exists<T: SafeDisplay>(error: T) -> Self {
        Self::AlreadyExists(Json(error.to_safe_string()))
    }
}

pub struct WorkerTraceErrorKind<'a>(pub &'a worker::v1::WorkerError);

impl Debug for WorkerTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for WorkerTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                worker::v1::worker_error::Error::BadRequest(_) => "BadRequest",
                worker::v1::worker_error::Error::Unauthorized(_) => "Unauthorized",
                worker::v1::worker_error::Error::LimitExceeded(_) => "LimitExceeded",
                worker::v1::worker_error::Error::NotFound(_) => "NotFound",
                worker::v1::worker_error::Error::AlreadyExists(_) => "AlreadyExists",
                worker::v1::worker_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}

pub struct ApiDefinitionTraceErrorKind<'a>(pub &'a ApiDefinitionError);

impl Debug for ApiDefinitionTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for ApiDefinitionTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                api_definition_error::Error::BadRequest(_) => "BadRequest",
                api_definition_error::Error::InvalidRoutes(_) => "InvalidRoutes",
                api_definition_error::Error::Unauthorized(_) => "Unauthorized",
                api_definition_error::Error::LimitExceeded(_) => "LimitExceeded",
                api_definition_error::Error::NotFound(_) => "NotFound",
                api_definition_error::Error::AlreadyExists(_) => "AlreadyExists",
                api_definition_error::Error::InternalError(_) => "InternalError",
                api_definition_error::Error::NotDraft(_) => "NotDraft",
            },
        }
    }
}

mod conversion {
    use super::{ApiEndpointError, ValidationErrorsBody, WorkerServiceErrorsBody};
    use crate::service::gateway::api_definition::ApiDefinitionError as ApiDefinitionServiceError;
    use crate::service::gateway::api_definition_validator::ValidationErrors;
    use crate::service::gateway::api_deployment::ApiDeploymentError;

    use crate::gateway_security::IdentityProviderError;
    use crate::service::gateway::security_scheme::SecuritySchemeServiceError;
    use golem_api_grpc::proto::golem::common::ErrorsBody;
    use golem_api_grpc::proto::golem::{
        apidefinition::v1::{api_definition_error, ApiDefinitionError, RouteValidationErrorsBody},
        common::ErrorBody,
    };
    use golem_common::{safe, SafeDisplay};
    use poem_openapi::payload::Json;
    use std::fmt::Display;

    impl From<SecuritySchemeServiceError> for ApiEndpointError {
        fn from(value: SecuritySchemeServiceError) -> Self {
            match value {
                SecuritySchemeServiceError::IdentityProviderError(identity_provider_error) => {
                    ApiEndpointError::from(identity_provider_error)
                }
                SecuritySchemeServiceError::InternalError(_) => ApiEndpointError::internal(value),
                SecuritySchemeServiceError::NotFound(_) => ApiEndpointError::not_found(value),
            }
        }
    }

    impl From<IdentityProviderError> for ApiEndpointError {
        fn from(value: IdentityProviderError) -> Self {
            match value {
                IdentityProviderError::ClientInitError(error) => {
                    ApiEndpointError::internal(safe(error))
                }
                IdentityProviderError::InvalidIssuerUrl(error) => {
                    ApiEndpointError::bad_request(safe(error))
                }
                IdentityProviderError::FailedToDiscoverProviderMetadata(error) => {
                    ApiEndpointError::bad_request(safe(error))
                }
                IdentityProviderError::FailedToExchangeCodeForTokens(error) => {
                    ApiEndpointError::unauthorized(safe(error))
                }
                IdentityProviderError::IdTokenVerificationError(error) => {
                    ApiEndpointError::unauthorized(safe(error))
                }
            }
        }
    }

    impl From<ApiDefinitionServiceError> for ApiEndpointError {
        fn from(error: ApiDefinitionServiceError) -> Self {
            match error {
                ApiDefinitionServiceError::ValidationError(e) => e.into(),
                ApiDefinitionServiceError::ComponentNotFoundError(_) => {
                    ApiEndpointError::bad_request(error)
                }
                ApiDefinitionServiceError::ApiDefinitionNotDraft(_) => {
                    ApiEndpointError::bad_request(error)
                }
                ApiDefinitionServiceError::ApiDefinitionNotFound(_) => {
                    ApiEndpointError::not_found(error)
                }
                ApiDefinitionServiceError::ApiDefinitionAlreadyExists(_) => {
                    ApiEndpointError::already_exists(error)
                }
                ApiDefinitionServiceError::ApiDefinitionDeployed(_) => {
                    ApiEndpointError::bad_request(error)
                }
                ApiDefinitionServiceError::RibCompilationErrors(_) => {
                    ApiEndpointError::bad_request(error)
                }
                ApiDefinitionServiceError::InternalRepoError(_) => {
                    ApiEndpointError::internal(error)
                }
                ApiDefinitionServiceError::SecuritySchemeError(error) => {
                    ApiEndpointError::from(error)
                }
                ApiDefinitionServiceError::IdentityProviderError(error) => {
                    ApiEndpointError::from(error)
                }
                ApiDefinitionServiceError::Internal(_) => ApiEndpointError::internal(error),
            }
        }
    }

    impl<Namespace: Display> From<ApiDeploymentError<Namespace>> for ApiEndpointError {
        fn from(error: ApiDeploymentError<Namespace>) -> Self {
            match error {
                ApiDeploymentError::ApiDefinitionNotFound(_, _) => {
                    ApiEndpointError::not_found(error)
                }
                ApiDeploymentError::ApiDeploymentNotFound(_, _) => {
                    ApiEndpointError::not_found(error)
                }
                ApiDeploymentError::ApiDeploymentConflict(_) => {
                    ApiEndpointError::already_exists(error)
                }
                ApiDeploymentError::ApiDefinitionsConflict(_) => {
                    ApiEndpointError::bad_request(error)
                }
                ApiDeploymentError::InternalRepoError(_) => ApiEndpointError::internal(error),
                ApiDeploymentError::InternalConversionError { .. } => {
                    ApiEndpointError::internal(error)
                }
                ApiDeploymentError::ComponentConstraintCreateError(_) => {
                    ApiEndpointError::internal(error)
                }
            }
        }
    }

    impl From<ValidationErrors> for ApiEndpointError {
        fn from(error: ValidationErrors) -> Self {
            let error = WorkerServiceErrorsBody::Validation(ValidationErrorsBody {
                errors: error.errors,
            });

            ApiEndpointError::BadRequest(Json(error))
        }
    }

    impl From<ApiDefinitionServiceError> for ApiDefinitionError {
        fn from(error: ApiDefinitionServiceError) -> ApiDefinitionError {
            match error {
                ApiDefinitionServiceError::ValidationError(e) => {
                    let errors = e.errors;

                    ApiDefinitionError {
                        error: Some(api_definition_error::Error::InvalidRoutes(
                            RouteValidationErrorsBody { errors },
                        )),
                    }
                }
                ApiDefinitionServiceError::SecuritySchemeError(error) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::IdentityProviderError(error) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::RibCompilationErrors(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionNotFound(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionNotDraft(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotDraft(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionAlreadyExists(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::AlreadyExists(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionDeployed(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::BadRequest(ErrorsBody {
                        errors: vec![error.to_safe_string()],
                    })),
                },
                ApiDefinitionServiceError::ComponentNotFoundError(error) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: format!(
                            "Components not found: {}",
                            error
                                .iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<String>>()
                                .join(", ")
                        ),
                    })),
                },
                ApiDefinitionServiceError::InternalRepoError(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::InternalError(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
                ApiDefinitionServiceError::Internal(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::InternalError(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                },
            }
        }
    }
}
