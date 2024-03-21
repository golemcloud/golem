use golem_api_grpc::proto::golem::worker::{
    worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};
use golem_common::model::{AccountId, TemplateId, WorkerId};
use golem_service_base::{
    model::{GolemError, VersionedTemplateId},
    service::auth::AuthError,
};

use crate::service::error::TemplateServiceError;

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error(transparent)]
    Template(#[from] TemplateServiceError),
    // TODO: This should prob be a vec?
    #[error("Type checker error: {0}")]
    TypeChecker(String),
    #[error("Template not found: {0}")]
    VersionedTemplateIdNotFound(VersionedTemplateId),
    #[error("Template not found: {0}")]
    TemplateNotFound(TemplateId),
    #[error("Account not found: {0}")]
    AccountIdNotFound(AccountId),
    // TODO: Once worker is independent of account
    #[error("Worker not found: {0}")]
    WorkerNotFound(WorkerId),
    // TODO: Fix display impl.
    #[error("Golem error")]
    Golem(GolemError),
}

impl WorkerServiceError {
    pub fn internal<M>(error: M) -> Self
    where
        M: std::error::Error + Send + Sync + 'static,
    {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl From<WorkerServiceError> for GrpcWorkerError {
    fn from(error: WorkerServiceError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}

impl From<WorkerServiceError> for worker_error::Error {
    fn from(value: WorkerServiceError) -> Self {
        match value {
            WorkerServiceError::Auth(error) => match error {
                AuthError::Unauthorized(error) => worker_error::Error::Unauthorized(
                    golem_api_grpc::proto::golem::common::ErrorBody {
                        error: error.to_string(),
                    },
                ),
                AuthError::Forbidden(error) => worker_error::Error::LimitExceeded(
                    golem_api_grpc::proto::golem::common::ErrorBody {
                        error: error.to_string(),
                    },
                ),
                AuthError::Internal(error) => worker_error::Error::InternalError(
                    golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: error.to_string(),
                        })),
                    },
                ),
            },
            WorkerServiceError::TemplateNotFound(template_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Template not found: {template_id}"),
                })
            }
            WorkerServiceError::AccountIdNotFound(account_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Account not found: {account_id}"),
                })
            }
            WorkerServiceError::VersionedTemplateIdNotFound(template_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Versioned template not found: {template_id}"),
                })
            }
            WorkerServiceError::WorkerNotFound(worker_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Worker not found: {worker_id}"),
                })
            }
            WorkerServiceError::Internal(error) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                },
            ),
            WorkerServiceError::TypeChecker(error) => {
                worker_error::Error::BadRequest(golem_api_grpc::proto::golem::common::ErrorsBody {
                    errors: vec![error],
                })
            }
            WorkerServiceError::Template(template) => template.into(),
            WorkerServiceError::Golem(worker_execution_error) => {
                worker_error::Error::InternalError(worker_execution_error.into())
            }
        }
    }
}
