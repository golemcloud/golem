use golem_api_grpc::proto::golem::worker::{
    worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};
use golem_common::model::{AccountId, TemplateId, WorkerId};
use golem_service_base::model::{GolemError, VersionedTemplateId};

use crate::service::template::TemplateServiceError;

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceError {
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
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
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
    fn from(error: WorkerServiceError) -> Self {
        use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
        use golem_api_grpc::proto::golem::worker::WorkerExecutionError;

        match error {
            error @ (WorkerServiceError::TemplateNotFound(_)
            | WorkerServiceError::AccountIdNotFound(_)
            | WorkerServiceError::VersionedTemplateIdNotFound(_)
            | WorkerServiceError::WorkerNotFound(_)) => worker_error::Error::NotFound(ErrorBody {
                error: error.to_string(),
            }),
            WorkerServiceError::Internal(_) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                })
            }
            WorkerServiceError::TypeChecker(error) => worker_error::Error::BadRequest(ErrorsBody {
                errors: vec![error],
            }),
            WorkerServiceError::Template(template) => template.into(),
            WorkerServiceError::Golem(worker_execution_error) => {
                worker_error::Error::InternalError(worker_execution_error.into())
            }
        }
    }
}
