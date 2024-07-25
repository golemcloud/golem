use std::fmt::{Debug, Display, Formatter};

use crate::service::http::http_api_definition_validator::RouteValidationError;
use golem_api_grpc::proto::golem::apidefinition::{api_definition_error, ApiDefinitionError};
use golem_api_grpc::proto::golem::worker;
use golem_common::metrics::api::TraceErrorKind;
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
    errors: Vec<RouteValidationError>,
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

pub struct WorkerTraceErrorKind<'a>(pub &'a worker::WorkerError);

impl<'a> Debug for WorkerTraceErrorKind<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'a> TraceErrorKind for WorkerTraceErrorKind<'a> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                worker::worker_error::Error::BadRequest(_) => "BadRequest",
                worker::worker_error::Error::Unauthorized(_) => "Unauthorized",
                worker::worker_error::Error::LimitExceeded(_) => "LimitExceeded",
                worker::worker_error::Error::NotFound(_) => "NotFound",
                worker::worker_error::Error::AlreadyExists(_) => "AlreadyExists",
                worker::worker_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}

pub struct ApiDefinitionTraceErrorKind<'a>(pub &'a ApiDefinitionError);

impl<'a> Debug for ApiDefinitionTraceErrorKind<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'a> TraceErrorKind for ApiDefinitionTraceErrorKind<'a> {
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
    use crate::service::api_definition::ApiDefinitionError as ApiDefinitionServiceError;
    use crate::service::api_definition_validator::ValidationErrors;
    use crate::service::api_deployment::ApiDeploymentError;
    use crate::service::http::http_api_definition_validator::RouteValidationError;
    use golem_api_grpc::proto::golem::common::ErrorsBody;
    use golem_api_grpc::proto::golem::{
        apidefinition,
        apidefinition::{api_definition_error, ApiDefinitionError, RouteValidationErrorsBody},
        common::ErrorBody,
    };
    use poem_openapi::payload::Json;
    use std::fmt::Display;

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
                ApiDefinitionServiceError::ApiDefinitionDeployed(_) => {
                    ApiEndpointError::bad_request(error)
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
                e @ ApiDeploymentError::ApiDefinitionsConflict(_) => {
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
                ApiDefinitionServiceError::ApiDefinitionDeployed(_) => ApiDefinitionError {
                    error: Some(api_definition_error::Error::BadRequest(ErrorsBody {
                        errors: vec![error.to_string()],
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
