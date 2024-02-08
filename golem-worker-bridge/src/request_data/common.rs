use std::fmt::Display;

use cloud_api_grpc::proto::golem::cloud::project::project_error::Error;
use golem_common::model::TemplateId;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Object, Tags, Union};
use serde::{Deserialize, Serialize};

use crate::api_validator;
use crate::apispec::MethodPattern;
use crate::certificate::CertificateError;
use crate::domain_record::RegisterDomainRouteError;
use crate::domain_register::RegisterDomainError;
use crate::project::ProjectError;

#[derive(Tags)]
pub enum ApiTags {
    ApiDefinition,
    ApiDeployment,
    ApiDomain,
    ApiCertificate,
    Healthcheck,
}

#[derive(Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum ErrorsBody {
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
pub struct ErrorBody {
    error: String,
}

#[derive(Object)]
pub struct MessageBody {
    message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteValidationError {
    pub method: MethodPattern,
    pub path: String,
    pub template: TemplateId,
    pub detail: String,
}

impl From<api_validator::RouteValidationError> for RouteValidationError {
    fn from(error: api_validator::RouteValidationError) -> Self {
        let path = error.path.to_string();
        Self {
            method: error.method,
            path,
            template: error.template,
            detail: error.detail,
        }
    }
}

#[derive(ApiResponse)]
pub enum ApiEndpointError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
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
        Self::BadRequest(Json(ErrorsBody::Messages(MessagesErrorsBody {
            errors: vec![error.to_string()],
        })))
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

impl From<ProjectError> for ApiEndpointError {
    fn from(value: ProjectError) -> Self {
        match value {
            ProjectError::Server(error) => match &error.error {
                None => ApiEndpointError::InternalError(Json(ErrorBody {
                    error: "Unknown project error".to_string(),
                })),
                Some(Error::BadRequest(errors)) => {
                    ApiEndpointError::BadRequest(Json(ErrorsBody::Messages(MessagesErrorsBody {
                        errors: errors.errors.clone(),
                    })))
                }
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
            RegisterDomainRouteError::NotAvailable(error) => {
                ApiEndpointError::BadRequest(Json(ErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![error],
                })))
            }
            RegisterDomainRouteError::Internal(error) => {
                ApiEndpointError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<RegisterDomainError> for ApiEndpointError {
    fn from(value: RegisterDomainError) -> Self {
        match value {
            RegisterDomainError::NotAvailable(error) => {
                ApiEndpointError::BadRequest(Json(ErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![error],
                })))
            }
            RegisterDomainError::Internal(error) => {
                ApiEndpointError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<CertificateError> for ApiEndpointError {
    fn from(value: CertificateError) -> Self {
        match value {
            CertificateError::NotAvailable(error) => {
                ApiEndpointError::BadRequest(Json(ErrorsBody::Messages(MessagesErrorsBody {
                    errors: vec![error],
                })))
            }
            CertificateError::Internal(error) => {
                ApiEndpointError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<api_validator::ValidationError> for ApiEndpointError {
    fn from(value: api_validator::ValidationError) -> Self {
        ApiEndpointError::BadRequest(Json(ErrorsBody::Validation(ValidationErrorsBody {
            errors: value
                .errors
                .iter()
                .map(|error| RouteValidationError::from(error.clone()))
                .collect(),
        })))
    }
}
