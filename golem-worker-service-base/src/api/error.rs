use poem_openapi::payload::Json;
use poem_openapi::*;
use tonic::Status;

use golem_service_base::model::*;

use crate::service::component::ComponentServiceError;
use crate::service::worker::WorkerServiceError;

// The dependents og golem-worker-service-base
// is expected to exposer worker api endpoints
// that can rely on WorkerApiBaseError
// If there are deviations from this (such as extra terms)
// it should be wrapping WorkerApiBaseError instead of repeating
// error types all over the place
#[derive(ApiResponse)]
pub enum WorkerApiBaseError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<GolemErrorBody>),
}

impl From<tonic::transport::Error> for WorkerApiBaseError {
    fn from(value: tonic::transport::Error) -> Self {
        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<Status> for WorkerApiBaseError {
    fn from(value: Status) -> Self {
        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<String> for WorkerApiBaseError {
    fn from(value: String) -> Self {
        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown { details: value }),
        }))
    }
}

impl From<WorkerServiceError> for WorkerApiBaseError {
    fn from(error: WorkerServiceError) -> Self {
        use WorkerServiceError as ServiceError;

        fn internal(details: String) -> WorkerApiBaseError {
            WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown { details }),
            }))
        }

        match error {
            ServiceError::Internal(_) => internal(error.to_string()),
            ServiceError::TypeChecker(_) => WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                errors: vec![error.to_string()],
            })),
            ServiceError::VersionedComponentIdNotFound(_)
            | ServiceError::ComponentNotFound(_)
            | ServiceError::AccountIdNotFound(_)
            | ServiceError::WorkerNotFound(_) => WorkerApiBaseError::NotFound(Json(ErrorBody {
                error: error.to_string(),
            })),
            ServiceError::Golem(golem_error) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody { golem_error }))
            }
            ServiceError::Component(error) => error.into(),
        }
    }
}

impl From<ComponentServiceError> for WorkerApiBaseError {
    fn from(value: ComponentServiceError) -> Self {
        match value {
            ComponentServiceError::BadRequest(errors) => {
                WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors }))
            }
            ComponentServiceError::AlreadyExists(error) => {
                WorkerApiBaseError::AlreadyExists(Json(ErrorBody { error }))
            }
            ComponentServiceError::Internal(error) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: error.to_string(),
                    }),
                }))
            }

            ComponentServiceError::NotFound(error) => {
                WorkerApiBaseError::NotFound(Json(ErrorBody { error }))
            }
            ComponentServiceError::Unauthorized(error) => {
                WorkerApiBaseError::Unauthorized(Json(ErrorBody { error }))
            }
            ComponentServiceError::Forbidden(error) => {
                WorkerApiBaseError::Forbidden(Json(ErrorBody { error }))
            }
        }
    }
}
