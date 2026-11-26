// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::service::auth::AuthServiceError;
use crate::service::component::ComponentServiceError;
use crate::service::limit::LimitServiceError;
use crate::service::worker::CallWorkerExecutorError;
use golem_common::SafeDisplay;
use golem_common::base_model::api;
use golem_common::model::account::AccountId;
use golem_common::model::component::{CanonicalFilePath, ComponentId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::filesystem::FileReadError;
use golem_common::model::{AgentId, OwnedInvocationId};
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthorizationError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedInvocation {
    pub id: OwnedInvocationId,
    pub environment_id: EnvironmentId,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceError {
    #[error("{source}")]
    AcceptedInvocation {
        accepted: AcceptedInvocation,
        source: Box<WorkerServiceError>,
    },
    #[error(transparent)]
    Component(#[from] ComponentServiceError),
    #[error(transparent)]
    LimitError(#[from] LimitServiceError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
    #[error(transparent)]
    InternalCallError(CallWorkerExecutorError),
    #[error(transparent)]
    GolemError(#[from] WorkerExecutorError),
    #[error("Type checker error: {0}")]
    TypeChecker(String),
    #[error("Component not found: {0}")]
    ComponentNotFound(ComponentId),
    #[error("Account not found: {0}")]
    AccountIdNotFound(AccountId),
    #[error("Worker not found: {0}")]
    AgentNotFound(AgentId),
    #[error("Invocation not found: {0}")]
    InvocationNotFound(String),
    /// Each denied request to an agent gets this text, whether the agent exists or not, so a
    /// denial does not tell a caller whether the agent exists or which component owns it.
    #[error("Access to agent {0} is denied")]
    AgentAccessDenied(AgentId),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("File not found: {0}")]
    FileNotFound(CanonicalFilePath),
    #[error("Bad file type: {0}")]
    BadFileType(CanonicalFilePath),
    #[error(transparent)]
    FileRead(#[from] FileReadError),
    #[error(transparent)]
    RegistryServiceError(#[from] RegistryServiceError),
}

impl SafeDisplay for WorkerServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AcceptedInvocation { source, .. } => source.to_safe_string(),
            Self::Component(inner) => inner.to_safe_string(),
            Self::TypeChecker(_) => self.to_string(),
            Self::ComponentNotFound(_) => self.to_string(),
            Self::AccountIdNotFound(_) => self.to_string(),
            Self::AgentNotFound(_) => self.to_string(),
            Self::InvocationNotFound(_) => self.to_string(),
            Self::AgentAccessDenied(_) => self.to_string(),
            Self::Internal(_) => self.to_string(),
            Self::GolemError(inner) => inner.to_safe_string(),
            Self::InternalCallError(inner) => inner.to_safe_string(),
            Self::FileNotFound(_) => self.to_string(),
            Self::BadFileType(_) => self.to_string(),
            Self::FileRead(_) => self.to_string(),
            Self::LimitError(inner) => inner.to_safe_string(),
            Self::AuthError(inner) => inner.to_safe_string(),
            Self::RegistryServiceError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<AuthorizationError> for WorkerServiceError {
    fn from(error: AuthorizationError) -> Self {
        Self::AuthError(AuthServiceError::Unauthorized(error))
    }
}

impl From<WorkerServiceError> for golem_api_grpc::proto::golem::worker::v1::AgentError {
    fn from(error: WorkerServiceError) -> Self {
        Self {
            error: Some(error.into()),
        }
    }
}

impl From<WorkerServiceError> for golem_api_grpc::proto::golem::worker::v1::agent_error::Error {
    fn from(error: WorkerServiceError) -> Self {
        use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
        use golem_api_grpc::proto::golem::worker::v1::UnknownError;
        use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
        use golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error as GrpcError;

        match error {
            WorkerServiceError::AcceptedInvocation { source, .. } => (*source).into(),
            WorkerServiceError::ComponentNotFound(_) => Self::NotFound(ErrorBody {
                error: error.to_safe_string(),
                code: api::error_code::COMPONENT_NOT_FOUND.to_string(),
            }),
            WorkerServiceError::AccountIdNotFound(_) => Self::NotFound(ErrorBody {
                error: error.to_safe_string(),
                code: api::error_code::ACCOUNT_NOT_FOUND.to_string(),
            }),
            WorkerServiceError::AgentNotFound(_) => Self::NotFound(ErrorBody {
                error: error.to_safe_string(),
                code: api::error_code::AGENT_NOT_FOUND.to_string(),
            }),
            WorkerServiceError::InvocationNotFound(_) => Self::NotFound(ErrorBody {
                error: error.to_safe_string(),
                code: api::error_code::INVOCATION_NOT_FOUND.to_string(),
            }),
            WorkerServiceError::FileNotFound(_) => Self::NotFound(ErrorBody {
                error: error.to_safe_string(),
                code: api::error_code::FILE_NOT_FOUND.to_string(),
            }),
            WorkerServiceError::RegistryServiceError(RegistryServiceError::NotFound(_)) => {
                Self::NotFound(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::RESOURCE_NOT_FOUND.to_string(),
                })
            }
            WorkerServiceError::Component(ComponentServiceError::ComponentNotFound) => {
                Self::NotFound(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::COMPONENT_NOT_FOUND.to_string(),
                })
            }
            WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { .. }) => {
                Self::NotFound(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::AGENT_NOT_FOUND.to_string(),
                })
            }
            WorkerServiceError::GolemError(WorkerExecutorError::ComponentNotFound { .. }) => {
                Self::NotFound(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::COMPONENT_NOT_FOUND.to_string(),
                })
            }
            WorkerServiceError::GolemError(WorkerExecutorError::PermissionDenied { .. }) => {
                Self::Unauthorized(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::AUTH_UNAUTHORIZED.to_string(),
                })
            }
            WorkerServiceError::GolemError(WorkerExecutorError::InvalidRequest { ref details })
                if details.starts_with("IdempotencyConflict:") =>
            {
                Self::BadRequest(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                    code: api::error_code::IDEMPOTENCY_CONFLICT.to_string(),
                })
            }

            WorkerServiceError::GolemError(WorkerExecutorError::StaleCursor { .. }) => {
                Self::BadRequest(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                    code: api::error_code::STALE_CURSOR.to_string(),
                })
            }
            WorkerServiceError::BadFileType(_) => Self::BadRequest(ErrorsBody {
                errors: vec![error.to_safe_string()],
                code: api::error_code::BAD_FILE_TYPE.to_string(),
            }),
            WorkerServiceError::FileRead(
                FileReadError::InvalidTarget | FileReadError::InvalidSelection,
            ) => Self::BadRequest(ErrorsBody {
                errors: vec![error.to_safe_string()],
                code: api::error_code::VALIDATION_ERROR.to_string(),
            }),
            WorkerServiceError::FileRead(_) => Self::InternalError(WorkerExecutionError {
                error: Some(GrpcError::Unknown(UnknownError {
                    details: error.to_safe_string(),
                })),
            }),
            WorkerServiceError::TypeChecker(_) => Self::BadRequest(ErrorsBody {
                errors: vec![error.to_safe_string()],
                code: api::error_code::TYPE_CHECKER_ERROR.to_string(),
            }),
            WorkerServiceError::RegistryServiceError(RegistryServiceError::BadRequest(_)) => {
                Self::BadRequest(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                    code: api::error_code::VALIDATION_ERROR.to_string(),
                })
            }

            WorkerServiceError::LimitError(LimitServiceError::LimitExceeded(_)) => {
                Self::LimitExceeded(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::LIMIT_EXCEEDED.to_string(),
                })
            }
            WorkerServiceError::RegistryServiceError(RegistryServiceError::LimitExceeded(_)) => {
                Self::LimitExceeded(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::LIMIT_EXCEEDED.to_string(),
                })
            }

            WorkerServiceError::GolemError(worker_execution_error) => {
                Self::InternalError(worker_execution_error.into())
            }

            // An allowed operation whose audit record cannot be written did not run. The
            // executor's worker proxy reads an internal error as transient and tries the call
            // again, so a short audit outage never becomes a final answer in the caller's oplog.
            WorkerServiceError::AuthError(AuthServiceError::Unauthorized(
                AuthorizationError::AuditUnavailable(_),
            )) => Self::InternalError(WorkerExecutionError {
                error: Some(GrpcError::Unknown(UnknownError {
                    details: error.to_safe_string(),
                })),
            }),
            WorkerServiceError::AuthError(_) | WorkerServiceError::AgentAccessDenied(_) => {
                Self::Unauthorized(ErrorBody {
                    error: error.to_safe_string(),
                    code: api::error_code::AUTH_UNAUTHORIZED.to_string(),
                })
            }

            WorkerServiceError::Internal(_)
            | WorkerServiceError::RegistryServiceError(_)
            | WorkerServiceError::InternalCallError(_)
            | WorkerServiceError::LimitError(_)
            | WorkerServiceError::Component(ComponentServiceError::InternalError(_)) => {
                Self::InternalError(WorkerExecutionError {
                    error: Some(GrpcError::Unknown(UnknownError {
                        details: error.to_safe_string(),
                    })),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::worker::v1::UnknownError;
    use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
    use golem_api_grpc::proto::golem::worker::v1::agent_error::Error as GrpcAgentError;
    use golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error as GrpcError;
    use golem_service_base::model::auth::audit::AuditUnavailable;
    use test_r::test;

    fn over_grpc(error: AuthorizationError) -> GrpcAgentError {
        WorkerServiceError::AuthError(AuthServiceError::Unauthorized(error)).into()
    }

    #[test]
    fn an_unavailable_audit_is_an_internal_error_that_names_the_audit() {
        let error = over_grpc(AuthorizationError::AuditUnavailable(AuditUnavailable {
            retry_after: std::time::Duration::from_secs(5),
        }));

        match error {
            GrpcAgentError::InternalError(WorkerExecutionError {
                error: Some(GrpcError::Unknown(UnknownError { details })),
            }) => assert!(details.contains("audit record"), "{details}"),
            other => panic!("expected an internal error, got: {other:?}"),
        }
    }

    #[test]
    fn a_denial_stays_unauthorized() {
        let error = over_grpc(AuthorizationError::SystemOnlyActionNotAllowed(
            "test".to_string(),
        ));

        assert!(
            matches!(error, GrpcAgentError::Unauthorized(_)),
            "{error:?}"
        );
    }
}
