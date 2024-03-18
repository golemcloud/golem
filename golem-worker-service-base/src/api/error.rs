use crate::service::error::TemplateServiceBaseError;
use golem_service_base::model::*;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tonic::Status;

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
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<GolemErrorBody>),
}

type Result<T> = std::result::Result<T, WorkerApiBaseError>;

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

impl From<crate::service::error::WorkerServiceBaseError> for WorkerApiBaseError {
    fn from(value: crate::service::error::WorkerServiceBaseError) -> Self {
        use crate::service::error::WorkerServiceBaseError as ServiceError;

        match value {
            ServiceError::Internal(error) => WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error }),
            })),
            ServiceError::TypeCheckerError(error) => WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                errors: vec![format!("Type checker error: {error}")],
            })),
            ServiceError::VersionedTemplateIdNotFound(template_id) => {
                WorkerApiBaseError::NotFound(Json(ErrorBody {
                    error: format!("Template not found: {template_id}"),
                }))
            }
            ServiceError::TemplateNotFound(template_id) => WorkerApiBaseError::NotFound(Json(ErrorBody {
                error: format!("Template not found: {template_id}"),
            })),
            ServiceError::AccountIdNotFound(account_id) => WorkerApiBaseError::NotFound(Json(ErrorBody {
                error: format!("Account not found: {account_id}"),
            })),
            ServiceError::WorkerNotFound(worker_id) => WorkerApiBaseError::NotFound(Json(ErrorBody {
                error: format!("Worker not found: {worker_id}"),
            })),
            ServiceError::Golem(golem_error) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody { golem_error }))
            }
            ServiceError::DelegatedTemplateServiceError(error) => error.into(),
        }
    }
}

impl From<TemplateServiceBaseError> for WorkerApiBaseError {
    fn from(value: TemplateServiceBaseError) -> Self {
        match value {
            TemplateServiceBaseError::Connection(error) => WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown { details: format!("Internal connection error: {error}") }),
            })),
            TemplateServiceBaseError::Other(error) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: format!("Internal error: {error}") }),
                }))
            },
            TemplateServiceBaseError::Transport(_) => {
                WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: "Transport Error when connecting to template service".to_string() }),
                }))
            },
            TemplateServiceBaseError::Server(template_error) => {
                match template_error.error {
                    Some(error) => match error {
                        golem_api_grpc::proto::golem::template::template_error::Error::BadRequest(errors) => {
                            WorkerApiBaseError::BadRequest(Json(ErrorsBody {
                                errors: errors.errors,
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::InternalError(error) => {
                            WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::NotFound(error) => {
                            WorkerApiBaseError::NotFound(Json(ErrorBody {
                                error: error.error,
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::Unauthorized(error) => {
                            WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::LimitExceeded(error) => {
                            WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        }
                        golem_api_grpc::proto::golem::template::template_error::Error::AlreadyExists(error) => {
                            WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        }
                    }
                    None => {
                        WorkerApiBaseError::InternalError(Json(GolemErrorBody {
                            golem_error: GolemError::Unknown(GolemErrorUnknown { details: "Unknown error connecting to template service".to_string() }),
                        }))
                    }
                }
            }
        }
    }
}
