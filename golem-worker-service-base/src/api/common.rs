use std::fmt::Display;

use golem_common::model::TemplateId;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Object, Union};
use serde::{Deserialize, Serialize};

use crate::api_definition::MethodPattern;

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
pub struct WorkerServiceErrorBody {
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

#[derive(ApiResponse)]
pub enum ApiEndpointError {
    #[oai(status = 400)]
    BadRequest(Json<WorkerServiceErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<WorkerServiceErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<WorkerServiceErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<MessageBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<String>),
    #[oai(status = 500)]
    InternalError(Json<WorkerServiceErrorBody>),
}

impl ApiEndpointError {
    pub fn unauthorized<T: Display>(error: T) -> Self {
        Self::Unauthorized(Json(WorkerServiceErrorBody {
            error: error.to_string(),
        }))
    }

    pub fn forbidden<T: Display>(error: T) -> Self {
        Self::Forbidden(Json(WorkerServiceErrorBody {
            error: error.to_string(),
        }))
    }

    pub fn internal<T: Display>(error: T) -> Self {
        Self::InternalError(Json(WorkerServiceErrorBody {
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

mod conversion {
    use poem_openapi::payload::Json;

    use super::{
        ApiEndpointError, RouteValidationError, ValidationErrorsBody, WorkerServiceErrorsBody,
    };
    use crate::api_definition_repo::ApiRegistrationRepoError;
    use crate::auth::AuthError;
    use crate::service::api_definition::ApiRegistrationError;
    use crate::service::api_definition_validator::ValidationError;

    impl From<ApiRegistrationError> for ApiEndpointError {
        fn from(error: ApiRegistrationError) -> Self {
            match error {
                ApiRegistrationError::AuthenticationError(AuthError::Forbidden { .. }) => {
                    ApiEndpointError::forbidden(error)
                }
                ApiRegistrationError::AuthenticationError(AuthError::Unauthorized { .. }) => {
                    ApiEndpointError::unauthorized(error)
                }
                ApiRegistrationError::RepoError(ApiRegistrationRepoError::AlreadyExists(_)) => {
                    ApiEndpointError::already_exists(error)
                }
                ApiRegistrationError::RepoError(ApiRegistrationRepoError::InternalError(_)) => {
                    ApiEndpointError::internal(error)
                }
                ApiRegistrationError::ValidationError(e) => e.into(),
            }
        }
    }

    impl From<ValidationError> for ApiEndpointError {
        fn from(error: ValidationError) -> Self {
            let error = WorkerServiceErrorsBody::Validation(ValidationErrorsBody {
                errors: error
                    .errors
                    .into_iter()
                    .map(|e| RouteValidationError {
                        method: e.method,
                        path: e.path.to_string(),
                        template: e.template,
                        detail: e.detail,
                    })
                    .collect(),
            });

            ApiEndpointError::BadRequest(Json(error))
        }
    }
}
