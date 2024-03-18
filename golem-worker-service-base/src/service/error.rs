use golem_common::model::{AccountId, TemplateId};
use golem_service_base::model::*;
use golem_service_base::routing_table::RoutingTableError;
use std::fmt::Display;
use tonic::{Status, Streaming};

pub enum WorkerError {
    Internal(String),
    TypeCheckerError(String),
    DelegatedTemplateServiceError(TemplateError),
    VersionedTemplateIdNotFound(VersionedTemplateId),
    TemplateNotFound(TemplateId),
    AccountIdNotFound(AccountId),
    // FIXME: Once worker is independent of account
    WorkerNotFound(WorkerId),
    Golem(GolemError),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            WorkerError::Internal(ref string) => write!(f, "Internal error: {}", string),
            WorkerError::TypeCheckerError(ref string) => {
                write!(f, "Type checker error: {}", string)
            }
            WorkerError::DelegatedTemplateServiceError(ref error) => {
                write!(f, "Delegated template service error: {}", error)
            }
            WorkerError::VersionedTemplateIdNotFound(ref versioned_template_id) => write!(
                f,
                "Versioned template id not found: {}",
                versioned_template_id
            ),
            WorkerError::TemplateNotFound(ref template_id) => {
                write!(f, "Template not found: {}", template_id)
            }
            WorkerError::AccountIdNotFound(ref account_id) => {
                write!(f, "Account id not found: {}", account_id)
            }
            WorkerError::WorkerNotFound(ref worker_id) => {
                write!(f, "Worker not found: {}", worker_id)
            }
            WorkerError::Golem(ref error) => write!(f, "Golem error: {:?}", error),
        }
    }
}

impl From<RoutingTableError> for WorkerError {
    fn from(error: RoutingTableError) -> Self {
        WorkerError::Internal(format!("Unable to get routing table: {:?}", error))
    }
}

impl From<TemplateError> for WorkerError {
    fn from(error: TemplateError) -> Self {
        WorkerError::DelegatedTemplateServiceError(error)
    }
}

#[derive(Debug)]
pub enum TemplateError {
    Connection(Status),
    Transport(tonic::transport::Error),
    Server(golem_api_grpc::proto::golem::template::TemplateError),
    Other(String),
}

impl TemplateError {
    fn is_retriable(&self) -> bool {
        matches!(self, TemplateError::Connection(_))
    }
}

impl From<golem_api_grpc::proto::golem::template::TemplateError> for TemplateError {
    fn from(value: golem_api_grpc::proto::golem::template::TemplateError) -> Self {
        TemplateError::Server(value)
    }
}

impl From<Status> for TemplateError {
    fn from(value: Status) -> Self {
        TemplateError::Connection(value)
    }
}

impl From<tonic::transport::Error> for TemplateError {
    fn from(value: tonic::transport::Error) -> Self {
        TemplateError::Transport(value)
    }
}

impl From<String> for TemplateError {
    fn from(value: String) -> Self {
        TemplateError::Other(value)
    }
}

impl Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Server(err) => match &err.error {
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
            TemplateError::Connection(status) => write!(f, "Connection error: {status}"),
            TemplateError::Transport(error) => write!(f, "Transport error: {error}"),
            TemplateError::Other(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TemplateError {
    // TODO
    // fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    //     Some(&self.source)
    // }
}
