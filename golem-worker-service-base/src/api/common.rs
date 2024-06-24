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
    use std::fmt::Display;

    use crate::service::api_definition::ApiDefinitionError as ApiDefinitionServiceError;
    use crate::service::api_definition_validator::ValidationErrors;
    use crate::service::api_deployment::ApiDeploymentError;
    use crate::service::http::http_api_definition_validator::RouteValidationError;

    use super::{ApiEndpointError, ValidationErrorsBody, WorkerServiceErrorsBody};

    impl From<ApiDefinitionServiceError<RouteValidationError>> for ApiEndpointError {
        fn from(error: ApiDefinitionServiceError<RouteValidationError>) -> Self {
            match error {
                ApiDefinitionServiceError::ValidationError(e) => e.into(),
                e @ ApiDefinitionServiceError::ComponentNotFoundError(_) => {
                    ApiEndpointError::bad_request(e)
                }
                ApiDefinitionServiceError::InternalError(error) => {
                    ApiEndpointError::internal(error)
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
            }
        }
    }

    impl<Namespace: Display> From<ApiDeploymentError<Namespace>> for ApiEndpointError {
        fn from(error: ApiDeploymentError<Namespace>) -> Self {
            match error {
                ApiDeploymentError::InternalError(error) => ApiEndpointError::internal(error),
                e @ ApiDeploymentError::ApiDefinitionNotFound(_, _) => {
                    ApiEndpointError::not_found(e)
                }
                e @ ApiDeploymentError::ApiDeploymentNotFound(_, _) => {
                    ApiEndpointError::not_found(e)
                }
                e @ ApiDeploymentError::ApiDeploymentConflict(_) => {
                    ApiEndpointError::already_exists(e)
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
                        component: e.component,
                        detail: e.detail,
                    })
                    .collect(),
            });

            ApiEndpointError::BadRequest(Json(error))
        }
    }

    impl From<ApiDefinitionServiceError<RouteValidationError>> for ApiDefinitionError {
        fn from(error: ApiDefinitionServiceError<RouteValidationError>) -> ApiDefinitionError {
            match error {
                ApiDefinitionServiceError::ValidationError(e) => {
                    let errors = e
                        .errors
                        .into_iter()
                        .map(|r| apidefinition::RouteValidationError {
                            method: r.method.to_string(),
                            path: r.path.to_string(),
                            component: Some(r.component.into()),
                            detail: r.detail,
                        })
                        .collect();
                    ApiDefinitionError {
                        error: Some(api_definition_error::Error::InvalidRoutes(
                            RouteValidationErrorsBody { errors },
                        )),
                    }
                }
                ApiDefinitionServiceError::ComponentNotFoundError(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionNotFound(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotFound(ErrorBody {
                        error: error.to_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionNotDraft(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::NotDraft(ErrorBody {
                        error: error.to_string(),
                    })),
                },
                ApiDefinitionServiceError::ApiDefinitionAlreadyExists(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::AlreadyExists(ErrorBody {
                        error: error.to_string(),
                    })),
                },
                ApiDefinitionServiceError::InternalError(error) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::InternalError(ErrorBody {
                        error: error.to_string(),
                    })),
                },
            }
        }
    }
}
