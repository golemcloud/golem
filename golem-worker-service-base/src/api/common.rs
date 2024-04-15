use std::fmt::Display;

use golem_service_base::model::ErrorBody;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Object, Union};

use crate::service::http::http_api_definition_validator::RouteValidationError;

#[derive(Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum WorkerServiceErrorsBody {
    Messages(MessagesErrorsBody),
    Validation(ValidationErrorsBody),
}

// TODO: These should probably use golem_common ErrorBody and ErrorsBody instead.

#[derive(Object)]
pub struct MessagesErrorsBody {
    errors: Vec<String>,
}

#[derive(Object)]
pub struct ValidationErrorsBody {
    errors: Vec<RouteValidationError>,
}

#[derive(ApiResponse)]
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

impl ApiEndpointError {
    pub fn unauthorized<T: Display>(error: T) -> Self {
        Self::Unauthorized(Json(ErrorBody {
            error: error.to_string(),
        }))
    }

    pub fn forbidden<T: Display>(error: T) -> Self {
        Self::Forbidden(Json(ErrorBody {
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
        Self::NotFound(Json(ErrorBody {
            error: error.to_string(),
        }))
    }

    pub fn already_exists<T: Display>(error: T) -> Self {
        Self::AlreadyExists(Json(error.to_string()))
    }
}

mod conversion {
    use golem_api_grpc::proto::golem::{
        apidefinition,
        apidefinition::{api_definition_error, ApiDefinitionError, RouteValidationErrorsBody},
        common::ErrorBody,
    };
    use poem_openapi::payload::Json;

    use crate::repo::api_definition_repo::ApiRegistrationRepoError;
    use crate::service::api_definition::ApiRegistrationError;
    use crate::service::api_definition_validator::ValidationErrors;
    use crate::service::http::http_api_definition_validator::RouteValidationError;

    use super::{ApiEndpointError, ValidationErrorsBody, WorkerServiceErrorsBody};

    impl From<ApiRegistrationError<RouteValidationError>> for ApiEndpointError {
        fn from(error: ApiRegistrationError<RouteValidationError>) -> Self {
            match error {
                ApiRegistrationError::RepoError(error) => match error {
                    ApiRegistrationRepoError::AlreadyExists(_) => {
                        ApiEndpointError::already_exists(error)
                    }
                    ApiRegistrationRepoError::NotFound(_) => ApiEndpointError::not_found(error),
                    ApiRegistrationRepoError::Internal(_) => ApiEndpointError::internal(error),
                },
                ApiRegistrationError::ValidationError(e) => e.into(),
                e @ ApiRegistrationError::TemplateNotFoundError(_) => {
                    ApiEndpointError::bad_request(e)
                }
            }
        }
    }

    impl From<ValidationErrors<RouteValidationError>> for ApiEndpointError {
        fn from(error: ValidationErrors<RouteValidationError>) -> Self {
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

    impl From<ApiRegistrationError<RouteValidationError>> for ApiDefinitionError {
        fn from(error: ApiRegistrationError<RouteValidationError>) -> ApiDefinitionError {
            match error {
                ApiRegistrationError::RepoError(error) => match error {
                    ApiRegistrationRepoError::AlreadyExists(_) => ApiDefinitionError {
                        error: Some(api_definition_error::Error::AlreadyExists(ErrorBody {
                            error: error.to_string(),
                        })),
                    },
                    ApiRegistrationRepoError::Internal(_) => ApiDefinitionError {
                        error: Some(api_definition_error::Error::InternalError(ErrorBody {
                            error: error.to_string(),
                        })),
                    },
                    ApiRegistrationRepoError::NotFound(_) => ApiDefinitionError {
                        error: Some(api_definition_error::Error::NotFound(ErrorBody {
                            error: error.to_string(),
                        })),
                    },
                },
                ApiRegistrationError::ValidationError(e) => {
                    let errors = e
                        .errors
                        .into_iter()
                        .map(|r| apidefinition::RouteValidationError {
                            method: r.method.to_string(),
                            path: r.path.to_string(),
                            template: Some(r.template.into()),
                            detail: r.detail,
                        })
                        .collect();
                    ApiDefinitionError {
                        error: Some(api_definition_error::Error::InvalidRoutes(
                            RouteValidationErrorsBody { errors },
                        )),
                    }
                }
                ApiRegistrationError::TemplateNotFoundError(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_string(),
                    })),
                },
            }
        }
    }
}
