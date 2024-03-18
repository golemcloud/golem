use poem_openapi::payload::Json;
use poem_openapi::*;
use tonic::Status;
use golem_service_base::model::*;
use crate::service::error::TemplateError;

#[derive(ApiResponse)]
pub enum WorkerError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<GolemErrorBody>),
}

type Result<T> = std::result::Result<T, WorkerError>;

impl From<tonic::transport::Error> for WorkerError {
    fn from(value: tonic::transport::Error) -> Self {
        WorkerError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<Status> for WorkerError {
    fn from(value: Status) -> Self {
        WorkerError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<String> for WorkerError {
    fn from(value: String) -> Self {
        WorkerError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown { details: value }),
        }))
    }
}

impl From<crate::service::error::WorkerError> for WorkerError {
    fn from(value: crate::service::error::WorkerError) -> Self {
        use crate::service::error::WorkerError as ServiceError;

        match value {
            ServiceError::Internal(error) => WorkerError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error }),
            })),
            ServiceError::TypeCheckerError(error) => WorkerError::BadRequest(Json(ErrorsBody {
                errors: vec![format!("Type checker error: {error}")],
            })),
            ServiceError::VersionedTemplateIdNotFound(template_id) => {
                WorkerError::NotFound(Json(ErrorBody {
                    error: format!("Template not found: {template_id}"),
                }))
            }
            ServiceError::TemplateNotFound(template_id) => WorkerError::NotFound(Json(ErrorBody {
                error: format!("Template not found: {template_id}"),
            })),
            ServiceError::AccountIdNotFound(account_id) => WorkerError::NotFound(Json(ErrorBody {
                error: format!("Account not found: {account_id}"),
            })),
            ServiceError::WorkerNotFound(worker_id) => WorkerError::NotFound(Json(ErrorBody {
                error: format!("Worker not found: {worker_id}"),
            })),
            ServiceError::Golem(golem_error) => {
                WorkerError::InternalError(Json(GolemErrorBody { golem_error }))
            }
            ServiceError::DelegatedTemplateServiceError(error) => error.into(),
        }
    }
}

impl From<TemplateError> for WorkerError {
    fn from(value: TemplateError) -> Self {
        match value {
            TemplateError::Connection(error) => WorkerError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown { details: format!("Internal connection error: {error}") }),
            })),
            TemplateError::Other(error) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: format!("Internal error: {error}") }),
                }))
            },
            TemplateError::Transport(_) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: "Transport Error when connecting to template service".to_string() }),
                }))
            },
            TemplateError::Server(template_error) => {
                match template_error.error {
                    Some(error) => match error {
                        golem_api_grpc::proto::golem::template::template_error::Error::BadRequest(errors) => {
                            WorkerError::BadRequest(Json(ErrorsBody {
                                errors: errors.errors,
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::InternalError(error) => {
                            WorkerError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::NotFound(error) => {
                            WorkerError::NotFound(Json(ErrorBody {
                                error: error.error,
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::Unauthorized(error) => {
                            WorkerError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        },
                        golem_api_grpc::proto::golem::template::template_error::Error::LimitExceeded(error) => {
                            WorkerError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        }
                        golem_api_grpc::proto::golem::template::template_error::Error::AlreadyExists(error) => {
                            WorkerError::InternalError(Json(GolemErrorBody {
                                golem_error: GolemError::Unknown(GolemErrorUnknown { details: error.error }),
                            }))
                        }
                    }
                    None => {
                        WorkerError::InternalError(Json(GolemErrorBody {
                            golem_error: GolemError::Unknown(GolemErrorUnknown { details: "Unknown error connecting to template service".to_string() }),
                        }))
                    }
                }
            }
        }
    }
}
