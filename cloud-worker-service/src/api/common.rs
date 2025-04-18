use crate::service::api_certificate::CertificateServiceError;
use crate::service::api_domain::ApiDomainServiceError;
use crate::service::api_domain::RegisterDomainRouteError;
use crate::service::api_security::SecuritySchemeServiceError;
use cloud_api_grpc::proto::golem::cloud::project::v1::project_error::Error;
use cloud_common::auth::CloudNamespace;
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::clients::project::ProjectError;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::ErrorBody;
use golem_common::{safe, SafeDisplay};
use golem_worker_service_base::gateway_security::IdentityProviderError;
use golem_worker_service_base::service::gateway::api_definition::ApiDefinitionError as BaseApiDefinitionError;
use golem_worker_service_base::service::gateway::api_deployment::ApiDeploymentError;
use golem_worker_service_base::service::gateway::security_scheme::SecuritySchemeServiceError as BaseSecuritySchemeServiceError;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Object, Tags, Union};

#[allow(clippy::enum_variant_names)]
#[derive(Tags)]
pub enum ApiTags {
    ApiDefinition,
    ApiDeployment,
    ApiDomain,
    ApiCertificate,
    Worker,
}

#[derive(Union, Debug, Clone)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum WorkerServiceErrorsBody {
    Messages(MessagesErrorsBody),
    Validation(ValidationErrorsBody),
}

#[derive(Object, Debug, Clone)]
pub struct MessagesErrorsBody {
    errors: Vec<String>,
}

#[derive(Object, Debug, Clone)]
pub struct ValidationErrorsBody {
    errors: Vec<String>,
}

#[derive(Object, Debug, Clone)]
pub struct MessageBody {
    message: String,
}

#[derive(ApiResponse, Debug, Clone)]
pub enum ApiEndpointError {
    #[oai(status = 400)]
    BadRequest(Json<WorkerServiceErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<MessageBody>),
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
            ApiEndpointError::LimitExceeded(_) => "LimitExceeded",
            ApiEndpointError::Unauthorized(_) => "Unauthorized",
            ApiEndpointError::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            ApiEndpointError::BadRequest(_) => true,
            ApiEndpointError::NotFound(_) => true,
            ApiEndpointError::AlreadyExists(_) => true,
            ApiEndpointError::LimitExceeded(_) => true,
            ApiEndpointError::Unauthorized(_) => true,
            ApiEndpointError::InternalError(_) => false,
        }
    }
}

impl ApiEndpointError {
    pub fn unauthorized<T: SafeDisplay>(error: T) -> Self {
        Self::Unauthorized(Json(ErrorBody {
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
        Self::NotFound(Json(MessageBody {
            message: error.to_safe_string(),
        }))
    }

    pub fn already_exists<T: SafeDisplay>(error: T) -> Self {
        Self::AlreadyExists(Json(error.to_safe_string()))
    }
}

impl From<ApiDeploymentError<CloudNamespace>> for ApiEndpointError {
    fn from(value: ApiDeploymentError<CloudNamespace>) -> Self {
        match value {
            ApiDeploymentError::ApiDefinitionNotFound(_, _, _) => {
                ApiEndpointError::not_found(value)
            }
            ApiDeploymentError::ApiDeploymentNotFound(_, _) => ApiEndpointError::not_found(value),
            ApiDeploymentError::ApiDeploymentConflict(_) => ApiEndpointError::already_exists(value),
            ApiDeploymentError::ApiDefinitionsConflict(_) => ApiEndpointError::bad_request(value),
            ApiDeploymentError::InternalRepoError(_) => ApiEndpointError::internal(value),
            ApiDeploymentError::InternalConversionError { .. } => ApiEndpointError::internal(value),
            ApiDeploymentError::ComponentConstraintCreateError(_) => {
                ApiEndpointError::bad_request(value)
            }
        }
    }
}

impl From<BaseSecuritySchemeServiceError> for ApiEndpointError {
    fn from(value: BaseSecuritySchemeServiceError) -> Self {
        match value {
            BaseSecuritySchemeServiceError::IdentityProviderError(identity_provider_error) => {
                ApiEndpointError::from(identity_provider_error)
            }
            BaseSecuritySchemeServiceError::InternalError(_) => ApiEndpointError::internal(value),
            BaseSecuritySchemeServiceError::NotFound(_) => ApiEndpointError::not_found(value),
        }
    }
}

impl From<SecuritySchemeServiceError> for ApiEndpointError {
    fn from(value: SecuritySchemeServiceError) -> Self {
        match value {
            SecuritySchemeServiceError::Auth(error) => ApiEndpointError::from(error),
            SecuritySchemeServiceError::Base(error) => ApiEndpointError::from(error),
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

impl From<BaseApiDefinitionError> for ApiEndpointError {
    fn from(value: BaseApiDefinitionError) -> Self {
        match value {
            BaseApiDefinitionError::ValidationError(error) => {
                let errors = error.errors.into_iter().collect::<Vec<_>>();

                let error = ValidationErrorsBody { errors };

                ApiEndpointError::BadRequest(Json(WorkerServiceErrorsBody::Validation(error)))
            }
            BaseApiDefinitionError::ApiDefinitionNotDraft(_) => {
                ApiEndpointError::bad_request(value)
            }
            BaseApiDefinitionError::ApiDefinitionNotFound(_) => ApiEndpointError::not_found(value),
            BaseApiDefinitionError::ApiDefinitionAlreadyExists(_, _) => {
                ApiEndpointError::already_exists(value)
            }
            BaseApiDefinitionError::ComponentNotFoundError(_) => {
                ApiEndpointError::bad_request(value)
            }
            BaseApiDefinitionError::ApiDefinitionDeployed(_) => {
                ApiEndpointError::bad_request(value)
            }
            BaseApiDefinitionError::RibCompilationErrors(_) => ApiEndpointError::bad_request(value),
            BaseApiDefinitionError::InternalRepoError(_) => ApiEndpointError::internal(value),
            BaseApiDefinitionError::Internal(_) => ApiEndpointError::internal(value),
            BaseApiDefinitionError::SecuritySchemeError(error) => ApiEndpointError::from(error),
            BaseApiDefinitionError::IdentityProviderError(error) => ApiEndpointError::from(error),
            BaseApiDefinitionError::RibInternal(_) => ApiEndpointError::internal(value),
            BaseApiDefinitionError::InvalidOasDefinition(_) => ApiEndpointError::bad_request(value),
            BaseApiDefinitionError::UnsupportedRibInput(_) => ApiEndpointError::bad_request(value),
            BaseApiDefinitionError::RibStaticAnalysisError(_) => {
                ApiEndpointError::bad_request(value)
            }
            BaseApiDefinitionError::RibByteCodeGenerationError(_) => {
                ApiEndpointError::internal(value)
            }
            BaseApiDefinitionError::RibParseError(_) => ApiEndpointError::bad_request(value),
        }
    }
}

impl From<ProjectError> for ApiEndpointError {
    fn from(value: ProjectError) -> Self {
        match value {
            ProjectError::Server(error) => match &error.error {
                None => ApiEndpointError::InternalError(Json(ErrorBody {
                    error: "Unknown project error".to_string(),
                })),
                Some(Error::BadRequest(errors)) => ApiEndpointError::BadRequest(Json(
                    WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                        errors: errors.errors.clone(),
                    }),
                )),
                Some(Error::InternalError(error)) => {
                    ApiEndpointError::InternalError(Json(ErrorBody {
                        error: error.error.clone(),
                    }))
                }
                Some(Error::NotFound(error)) => ApiEndpointError::NotFound(Json(MessageBody {
                    message: error.error.clone(),
                })),
                Some(Error::Unauthorized(error)) => {
                    ApiEndpointError::Unauthorized(Json(ErrorBody {
                        error: error.error.clone(),
                    }))
                }
                Some(Error::LimitExceeded(error)) => {
                    ApiEndpointError::LimitExceeded(Json(ErrorBody {
                        error: error.error.clone(),
                    }))
                }
            },
            ProjectError::Connection(status) => ApiEndpointError::InternalError(Json(ErrorBody {
                error: format!("Project service connection error: {status}"),
            })),
            ProjectError::Transport(error) => ApiEndpointError::InternalError(Json(ErrorBody {
                error: format!("Project service transport error: {error}"),
            })),
            ProjectError::Unknown(_) => ApiEndpointError::InternalError(Json(ErrorBody {
                error: "Unknown project error".to_string(),
            })),
        }
    }
}

impl From<RegisterDomainRouteError> for ApiEndpointError {
    fn from(value: RegisterDomainRouteError) -> Self {
        match value {
            RegisterDomainRouteError::NotAvailable(_) => ApiEndpointError::BadRequest(Json(
                WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![value.to_safe_string()],
                }),
            )),
            RegisterDomainRouteError::AWSError { .. } => {
                ApiEndpointError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}

impl From<ApiDomainServiceError> for ApiEndpointError {
    fn from(value: ApiDomainServiceError) -> Self {
        match value {
            ApiDomainServiceError::Unauthorized(_) => {
                ApiEndpointError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            ApiDomainServiceError::NotFound(_) => ApiEndpointError::BadRequest(Json(
                WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![value.to_safe_string()],
                }),
            )),
            ApiDomainServiceError::AlreadyExists(_) => {
                ApiEndpointError::AlreadyExists(Json(value.to_safe_string()))
            }
            ApiDomainServiceError::InternalConversionError(_)
            | ApiDomainServiceError::InternalRepoError(_)
            | ApiDomainServiceError::InternalAuthClientError(_)
            | ApiDomainServiceError::InternalAWSError { .. } => {
                ApiEndpointError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}

impl From<CertificateServiceError> for ApiEndpointError {
    fn from(value: CertificateServiceError) -> Self {
        match value {
            CertificateServiceError::CertificateNotAvailable(_) => ApiEndpointError::BadRequest(
                Json(WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![value.to_safe_string()],
                })),
            ),
            CertificateServiceError::CertificateNotFound(_) => ApiEndpointError::BadRequest(Json(
                WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![value.to_safe_string()],
                }),
            )),
            CertificateServiceError::Unauthorized(_) => {
                ApiEndpointError::Unauthorized(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            CertificateServiceError::InternalCertificateManagerError(_)
            | CertificateServiceError::InternalAuthClientError(_)
            | CertificateServiceError::InternalRepoError(_)
            | CertificateServiceError::InternalConversionError(_) => {
                ApiEndpointError::InternalError(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
        }
    }
}

impl From<AuthServiceError> for ApiEndpointError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(_) => ApiEndpointError::Unauthorized(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AuthServiceError::Forbidden(_) => ApiEndpointError::LimitExceeded(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AuthServiceError::InternalClientError(_) => {
                ApiEndpointError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}
