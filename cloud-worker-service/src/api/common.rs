use golem_service_base::model::ErrorBody;
use std::fmt::Display;

use crate::service::api_certificate::CertificateServiceError;
use crate::service::api_definition::ApiDefinitionError;
use crate::service::api_domain::ApiDomainServiceError;
use crate::service::api_domain::RegisterDomainRouteError;
use crate::service::auth::{AuthServiceError, CloudNamespace};
use crate::service::project::ProjectError;
use cloud_api_grpc::proto::golem::cloud::project::project_error::Error;
use golem_worker_service_base::repo::api_definition_repo::ApiRegistrationRepoError;
use golem_worker_service_base::service::api_definition::ApiRegistrationError;
use golem_worker_service_base::service::http::http_api_definition_validator::RouteValidationError;
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

#[derive(Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum WorkerServiceErrorsBody {
    Messages(MessagesErrorsBody),
    Validation(ValidationErrorsBody),
}

#[derive(Object)]
pub struct MessagesErrorsBody {
    errors: Vec<String>,
}

#[derive(Object)]
pub struct ValidationErrorsBody {
    errors: Vec<RouteValidationError>,
}

#[derive(Object)]
pub struct MessageBody {
    message: String,
}

#[derive(ApiResponse)]
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

impl ApiEndpointError {
    pub fn unauthorized<T: Display>(error: T) -> Self {
        Self::Unauthorized(Json(ErrorBody {
            error: error.to_string(),
        }))
    }

    pub fn internal<T: Display>(error: T) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: error.to_string(),
        }))
    }

    pub fn bad_request<T: Display>(error: T) -> Self {
        Self::BadRequest(Json(WorkerServiceErrorsBody::Messages(
            MessagesErrorsBody {
                errors: vec![error.to_string()],
            },
        )))
    }

    pub fn not_found<T: Display>(error: T) -> Self {
        Self::NotFound(Json(MessageBody {
            message: error.to_string(),
        }))
    }

    pub fn already_exists<T: Display>(error: T) -> Self {
        Self::AlreadyExists(Json(error.to_string()))
    }
}

impl From<golem_worker_service_base::service::api_deployment::ApiDeploymentError<CloudNamespace>>
    for ApiEndpointError
{
    fn from(
        value: golem_worker_service_base::service::api_deployment::ApiDeploymentError<
            CloudNamespace,
        >,
    ) -> Self {
        match value {
            golem_worker_service_base::service::api_deployment::ApiDeploymentError::InternalError(error) => {
                ApiEndpointError::InternalError(Json(ErrorBody { error }))
            }
            golem_worker_service_base::service::api_deployment::ApiDeploymentError::ApiDefinitionNotFound(_, api_definition_id) => {
                ApiEndpointError::NotFound(Json(MessageBody {
                    message: format!("ApiDefinition not found id: {}", api_definition_id)
                }))
            }
            golem_worker_service_base::service::api_deployment::ApiDeploymentError::ApiDeploymentNotFound(_, site) => {
                ApiEndpointError::NotFound(Json(MessageBody {
                    message: format!("ApiDeployment nott found for site: {}", site)
                }))
            }

            golem_worker_service_base::service::api_deployment::ApiDeploymentError::DeploymentConflict(site) => {
                ApiEndpointError::AlreadyExists(Json(format!("Deployment conflict for site: {}", site)))
            }
        }
    }
}

impl From<ApiRegistrationError<RouteValidationError>> for ApiEndpointError {
    fn from(value: ApiRegistrationError<RouteValidationError>) -> Self {
        match value {
            ApiRegistrationError::ValidationError(error) => {
                let errors = error
                    .errors
                    .into_iter()
                    .map(|e| RouteValidationError {
                        method: e.method,
                        path: e.path,
                        component: e.component,
                        detail: e.detail,
                    })
                    .collect::<Vec<_>>();

                let error = ValidationErrorsBody { errors };

                ApiEndpointError::BadRequest(Json(WorkerServiceErrorsBody::Validation(error)))
            }
            ApiRegistrationError::RepoError(error) => match error {
                ApiRegistrationRepoError::AlreadyExists(_) => ApiEndpointError::AlreadyExists(
                    Json("ApiDefinition already exists".to_string()),
                ),
                ApiRegistrationRepoError::Internal(error) => {
                    ApiEndpointError::InternalError(Json(ErrorBody {
                        error: error.to_string(),
                    }))
                }
                ApiRegistrationRepoError::NotFound(definition_key) => {
                    ApiEndpointError::NotFound(Json(MessageBody {
                        message: format!("ApiDefinition not found: {}", definition_key.id),
                    }))
                }
                ApiRegistrationRepoError::NotDraft(e) => ApiEndpointError::bad_request(e),
            },
            error @ ApiRegistrationError::ComponentNotFoundError(_) => {
                ApiEndpointError::BadRequest(Json(WorkerServiceErrorsBody::Messages(
                    MessagesErrorsBody {
                        errors: vec![error.to_string()],
                    },
                )))
            }
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
            RegisterDomainRouteError::NotAvailable(error) => ApiEndpointError::BadRequest(Json(
                WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![error],
                }),
            )),
            RegisterDomainRouteError::Internal(error) => {
                ApiEndpointError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<ApiDomainServiceError> for ApiEndpointError {
    fn from(value: ApiDomainServiceError) -> Self {
        match value {
            ApiDomainServiceError::Unauthorized(_) => {
                ApiEndpointError::Unauthorized(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            ApiDomainServiceError::NotFound(_) => ApiEndpointError::BadRequest(Json(
                WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![value.to_string()],
                }),
            )),
            ApiDomainServiceError::AlreadyExists(_) => {
                ApiEndpointError::AlreadyExists(Json(value.to_string()))
            }
            ApiDomainServiceError::Internal(_) => {
                ApiEndpointError::InternalError(Json(ErrorBody {
                    error: value.to_string(),
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
                    errors: vec![value.to_string()],
                })),
            ),
            CertificateServiceError::CertificateNotFound(_) => ApiEndpointError::BadRequest(Json(
                WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![value.to_string()],
                }),
            )),
            CertificateServiceError::Unauthorized(_) => {
                ApiEndpointError::Unauthorized(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            CertificateServiceError::Internal(_) => {
                ApiEndpointError::InternalError(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
        }
    }
}

impl From<ApiDefinitionError> for ApiEndpointError {
    fn from(value: ApiDefinitionError) -> Self {
        match value {
            ApiDefinitionError::Auth(e) => e.into(),
            ApiDefinitionError::Base(e) => e.into(),
        }
    }
}

impl From<AuthServiceError> for ApiEndpointError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(error) => {
                ApiEndpointError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Forbidden(error) => {
                ApiEndpointError::LimitExceeded(Json(ErrorBody { error }))
            }
            AuthServiceError::Internal(error) => ApiEndpointError::InternalError(Json(ErrorBody {
                error: error.to_string(),
            })),
        }
    }
}
