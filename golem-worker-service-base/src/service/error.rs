use golem_common::model::{AccountId, TemplateId};
use golem_service_base::model::*;
use golem_service_base::routing_table::RoutingTableError;
use std::fmt::Display;
use tonic::Status;
use golem_api_grpc::proto::golem::worker::{UnknownError, worker_error, WorkerError as GrpcWorkerError, worker_execution_error};
// The dependents of golem-worker-service-base is expected
// to have a worker service that can depend on this base error
pub enum WorkerServiceBaseError {
    Internal(String),
    TypeCheckerError(String),
    DelegatedTemplateServiceError(TemplateServiceBaseError),
    VersionedTemplateIdNotFound(VersionedTemplateId),
    TemplateNotFound(TemplateId),
    AccountIdNotFound(AccountId),
    // FIXME: Once worker is independent of account
    WorkerNotFound(WorkerId),
    Golem(GolemError),
}

impl std::fmt::Display for WorkerServiceBaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            WorkerServiceBaseError::Internal(ref string) => write!(f, "Internal error: {}", string),
            WorkerServiceBaseError::TypeCheckerError(ref string) => {
                write!(f, "Type checker error: {}", string)
            }
            WorkerServiceBaseError::DelegatedTemplateServiceError(ref error) => {
                write!(f, "Delegated template service error: {}", error)
            }
            WorkerServiceBaseError::VersionedTemplateIdNotFound(ref versioned_template_id) => {
                write!(
                    f,
                    "Versioned template id not found: {}",
                    versioned_template_id
                )
            }
            WorkerServiceBaseError::TemplateNotFound(ref template_id) => {
                write!(f, "Template not found: {}", template_id)
            }
            WorkerServiceBaseError::AccountIdNotFound(ref account_id) => {
                write!(f, "Account id not found: {}", account_id)
            }
            WorkerServiceBaseError::WorkerNotFound(ref worker_id) => {
                write!(f, "Worker not found: {}", worker_id)
            }
            WorkerServiceBaseError::Golem(ref error) => write!(f, "Golem error: {:?}", error),
        }
    }
}

impl From<WorkerServiceBaseError> for worker_error::Error {
    fn from(value: WorkerServiceBaseError) -> Self {
        match value {
            WorkerServiceBaseError::TemplateNotFound(template_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Template not found: {template_id}"),
                })
            }
            WorkerServiceBaseError::AccountIdNotFound(account_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Account not found: {account_id}"),
                })
            }
            WorkerServiceBaseError::VersionedTemplateIdNotFound(template_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Versioned template not found: {template_id}"),
                })
            }
            WorkerServiceBaseError::WorkerNotFound(worker_id) => {
                worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody {
                    error: format!("Worker not found: {worker_id}"),
                })
            }
            WorkerServiceBaseError::Internal(error) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error,
                    })),
                },
            ),
            WorkerServiceBaseError::TypeCheckerError(error) => {
                worker_error::Error::BadRequest(golem_api_grpc::proto::golem::common::ErrorsBody {
                    errors: vec![error],
                })
            }
            WorkerServiceBaseError::DelegatedTemplateServiceError(_) => todo!(),
            WorkerServiceBaseError::Golem(worker_execution_error) => {
                worker_error::Error::InternalError(worker_execution_error.into())
            }
        }
    }
}

impl From<RoutingTableError> for WorkerServiceBaseError {
    fn from(error: RoutingTableError) -> Self {
        WorkerServiceBaseError::Internal(format!("Unable to get routing table: {:?}", error))
    }
}

impl From<TemplateServiceBaseError> for WorkerServiceBaseError {
    fn from(error: TemplateServiceBaseError) -> Self {
        WorkerServiceBaseError::DelegatedTemplateServiceError(error)
    }
}

// The dependents of golem-worker-service-base is expected
// to have a template service internally that can depend on this base error
#[derive(Debug)]
pub enum TemplateServiceBaseError {
    Connection(Status),
    Transport(tonic::transport::Error),
    Server(golem_api_grpc::proto::golem::template::TemplateError),
    Other(String),
}

impl TemplateServiceBaseError {
    pub fn is_retriable(&self) -> bool {
        matches!(self, TemplateServiceBaseError::Connection(_))
    }
}

impl From<golem_api_grpc::proto::golem::template::TemplateError> for TemplateServiceBaseError {
    fn from(value: golem_api_grpc::proto::golem::template::TemplateError) -> Self {
        TemplateServiceBaseError::Server(value)
    }
}

impl From<Status> for TemplateServiceBaseError {
    fn from(value: Status) -> Self {
        TemplateServiceBaseError::Connection(value)
    }
}

impl From<tonic::transport::Error> for TemplateServiceBaseError {
    fn from(value: tonic::transport::Error) -> Self {
        TemplateServiceBaseError::Transport(value)
    }
}

impl From<String> for TemplateServiceBaseError {
    fn from(value: String) -> Self {
        TemplateServiceBaseError::Other(value)
    }
}

impl Display for TemplateServiceBaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateServiceBaseError::Server(err) => match &err.error {
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
            TemplateServiceBaseError::Connection(status) => write!(f, "Connection error: {status}"),
            TemplateServiceBaseError::Transport(error) => write!(f, "Transport error: {error}"),
            TemplateServiceBaseError::Other(error) => write!(f, "{error}"),
        }
    }
}

impl From<TemplateServiceBaseError> for worker_error::Error {
    fn from(value: TemplateServiceBaseError) -> Self {
        match value {
            TemplateServiceBaseError::Connection(status) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: format!("Connection error:  Status: {status}"),
                    })),
                },
            ),
            TemplateServiceBaseError::Transport(transport_error) => {
                worker_error::Error::InternalError(
                    golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: format!("Transport error: {transport_error}"),
                        })),
                    },
                )
            }
            TemplateServiceBaseError::Server(template_error) => match template_error.error {
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::AlreadyExists(
                        error,
                    ),
                ) => worker_error::Error::AlreadyExists(error),

                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::BadRequest(
                        errors,
                    ),
                ) => worker_error::Error::BadRequest(golem_api_grpc::proto::golem::common::ErrorsBody {
                    errors: errors.errors,
                }),
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
                     )) => worker_error::Error::NotFound(golem_api_grpc::proto::golem::common::ErrorBody { error: error.error }),
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::Unauthorized(
                        error,
                    ),
                ) => worker_error::Error::Unauthorized(golem_api_grpc::proto::golem::common::ErrorBody { error: error.error }),
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::LimitExceeded(
                        error,
                    ),
                ) => worker_error::Error::LimitExceeded(golem_api_grpc::proto::golem::common::ErrorBody { error: error.error }),
                None => worker_error::Error::InternalError(
                    golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: "Unknown error".to_string(),
                        })),
                    },
                ),
            },
            TemplateServiceBaseError::Other(error) => worker_error::Error::InternalError(
                golem_api_grpc::proto::golem::worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: format!("Unknown error: {error}"),
                    })),
                },
            ),
        }
    }
}

impl std::error::Error for TemplateServiceBaseError {
    // TODO
    // fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    //     Some(&self.source)
    // }
}


impl From<WorkerServiceBaseError> for GrpcWorkerError {
    fn from(error: WorkerServiceBaseError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}


impl From<TemplateServiceBaseError> for GrpcWorkerError {
    fn from(error: TemplateServiceBaseError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}
