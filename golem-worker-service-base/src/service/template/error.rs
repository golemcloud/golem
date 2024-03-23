use golem_api_grpc::proto::golem::worker::{
    self, worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};
use golem_service_base::service::auth::AuthError;
use tonic::Status;

// The dependents of golem-worker-service-base is expected
// to have a template service internally that can depend on this base error
#[derive(Debug, thiserror::Error)]
pub enum TemplateServiceError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("Bad Request: {0:?}")]
    BadRequest(Vec<String>),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error("Not Found: {0}")]
    NotFound(String),
    #[error("Already Exists: {0}")]
    AlreadyExists(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl TemplateServiceError {
    pub fn internal<M>(error: M) -> Self
    where
        M: std::fmt::Display + std::fmt::Debug + Send + Sync + 'static,
    {
        Self::Internal(anyhow::Error::msg(error))
    }
    pub fn is_retriable(&self) -> bool {
        match self {
            TemplateServiceError::Internal(error) => error.is::<Status>(),
            _ => false,
        }
    }
}

impl From<Status> for TemplateServiceError {
    fn from(status: Status) -> Self {
        TemplateServiceError::Internal(status.into())
    }
}

impl From<tonic::transport::Error> for TemplateServiceError {
    fn from(error: tonic::transport::Error) -> Self {
        TemplateServiceError::Internal(error.into())
    }
}

impl From<golem_api_grpc::proto::golem::template::TemplateError> for TemplateServiceError {
    fn from(error: golem_api_grpc::proto::golem::template::TemplateError) -> Self {
        use golem_api_grpc::proto::golem::template::template_error::Error;
        match error.error {
            Some(Error::BadRequest(errors)) => TemplateServiceError::BadRequest(errors.errors),
            Some(Error::Unauthorized(error)) => TemplateServiceError::Unauthorized(error.error),
            Some(Error::LimitExceeded(error)) => TemplateServiceError::LimitExceeded(error.error),
            Some(Error::NotFound(error)) => TemplateServiceError::NotFound(error.error),
            Some(Error::AlreadyExists(error)) => TemplateServiceError::AlreadyExists(error.error),
            Some(Error::InternalError(error)) => {
                TemplateServiceError::Internal(anyhow::Error::msg(error.error))
            }
            None => TemplateServiceError::Internal(anyhow::Error::msg("Unknown error")),
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

impl From<TemplateServiceError> for worker_error::Error {
    fn from(value: TemplateServiceError) -> Self {
        use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};

        match value {
            TemplateServiceError::Auth(error) => match error {
                AuthError::Unauthorized(_) => worker_error::Error::Unauthorized(ErrorBody {
                    error: error.to_string(),
                }),
                AuthError::Forbidden(_) => worker_error::Error::LimitExceeded(ErrorBody {
                    error: error.to_string(),
                }),
                AuthError::NotFound(_) => worker_error::Error::NotFound(ErrorBody {
                    error: error.to_string(),
                }),
                AuthError::Internal(_) => {
                    worker_error::Error::InternalError(worker::WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: error.to_string(),
                        })),
                    })
                }
            },
            TemplateServiceError::AlreadyExists(error) => {
                worker_error::Error::AlreadyExists(ErrorBody { error })
            }
            TemplateServiceError::BadRequest(errors) => {
                worker_error::Error::BadRequest(ErrorsBody { errors })
            }
            TemplateServiceError::Unauthorized(error) => {
                worker_error::Error::Unauthorized(ErrorBody { error })
            }
            TemplateServiceError::LimitExceeded(error) => {
                worker_error::Error::LimitExceeded(ErrorBody { error })
            }
            TemplateServiceError::NotFound(error) => {
                worker_error::Error::NotFound(ErrorBody { error })
            }
            TemplateServiceError::Internal(error) => {
                worker_error::Error::InternalError(worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                })
            }
        }
    }
}
