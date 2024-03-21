use golem_api_grpc::proto::golem::worker::{
    worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};
use std::fmt::Display;
use tonic::Status;

// The dependents of golem-worker-service-base is expected
// to have a template service internally that can depend on this base error
#[derive(Debug, thiserror::Error)]
pub enum TemplateServiceError {
    Connection(#[from] Status),
    Transport(#[from] tonic::transport::Error),
    Server(golem_api_grpc::proto::golem::template::TemplateError),
    Internal(String),
}

impl TemplateServiceError {
    pub fn is_retriable(&self) -> bool {
        matches!(self, TemplateServiceError::Connection(_))
    }
}

impl From<golem_api_grpc::proto::golem::template::TemplateError> for TemplateServiceError {
    fn from(error: golem_api_grpc::proto::golem::template::TemplateError) -> Self {
        TemplateServiceError::Server(error)
    }
}

impl Display for TemplateServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateServiceError::Server(err) => match &err.error {
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::BadRequest(
                        errors,
                    ),
                ) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::InternalError(
                        error,
                    ),
                ) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(golem_api_grpc::proto::golem::template::template_error::Error::NotFound(
                    error,
                )) => {
                    write!(f, "Template not found: {}", error.error)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::Unauthorized(
                        error,
                    ),
                ) => {
                    write!(f, "Unauthorized: {}", error.error)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::LimitExceeded(
                        error,
                    ),
                ) => {
                    write!(f, "Template limit reached: {}", error.error)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::AlreadyExists(
                        error,
                    ),
                ) => {
                    write!(f, "Template already exists: {}", error.error)
                }
                None => write!(f, "Empty error response"),
            },
            TemplateServiceError::Connection(status) => write!(f, "Connection error: {status}"),
            TemplateServiceError::Transport(error) => write!(f, "Transport error: {error}"),
            TemplateServiceError::Internal(error) => write!(f, "{error}"),
        }
    }
}

impl From<TemplateServiceError> for worker_error::Error {
    fn from(value: TemplateServiceError) -> Self {
        match value {
            TemplateServiceError::Connection(status) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: format!("Connection error:  Status: {status}"),
                    })),
                },
            ),
            TemplateServiceError::Transport(transport_error) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: format!("Transport error: {transport_error}"),
                    })),
                },
            ),
            TemplateServiceError::Server(template_error) => match template_error.error {
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::AlreadyExists(
                        error,
                    ),
                ) => worker_error::Error::AlreadyExists(error),

                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::BadRequest(
                        errors,
                    ),
                ) => worker_error::Error::BadRequest(
                    golem_api_grpc::proto::golem::common::ErrorsBody {
                        errors: errors.errors,
                    },
                ),
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::InternalError(
                        error,
                    ),
                ) => {
                    let error0 = error.error;

                    worker_error::Error::InternalError(
                        golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                            error: Some(worker_execution_error::Error::Unknown(UnknownError {
                                details: format!("Template Internal error: {error0}"),
                            })),
                        },
                    )
                }
                Some(golem_api_grpc::proto::golem::template::template_error::Error::NotFound(
                    error,
                )) => {
                    worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                        error: error.error,
                    })
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::Unauthorized(
                        error,
                    ),
                ) => worker_error::Error::Unauthorized(
                    golem_api_grpc::proto::golem::common::ErrorBody { error: error.error },
                ),
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::LimitExceeded(
                        error,
                    ),
                ) => worker_error::Error::LimitExceeded(
                    golem_api_grpc::proto::golem::common::ErrorBody { error: error.error },
                ),
                None => worker_error::Error::InternalError(
                    golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: "Unknown error".to_string(),
                        })),
                    },
                ),
            },
            TemplateServiceError::Internal(error) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: format!("Unknown error: {error}"),
                    })),
                },
            ),
        }
    }
}

impl From<TemplateServiceError> for GrpcWorkerError {
    fn from(error: TemplateServiceError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}
