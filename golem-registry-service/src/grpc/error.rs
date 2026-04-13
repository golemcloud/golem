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

use crate::services::account_usage::error::{AccountUsageError, LimitExceededError};
use crate::services::auth::AuthError;
use crate::services::component::ComponentError;
use crate::services::component_resolver::ComponentResolverError;
use crate::services::deployment::{DeployedMcpError, DeployedRoutesError, DeploymentError};
use crate::services::environment::EnvironmentError;
use crate::services::environment_state::EnvironmentStateError;
use crate::services::resource_definition::ResourceDefinitionError;
use golem_common::IntoAnyhow;
use golem_common::base_model::api;
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_service_base::model::auth::AuthorizationError;

#[derive(Debug)]
pub enum GrpcApiError {
    BadRequest(ErrorsBody),
    Unauthorized(ErrorBody),
    LimitExceeded(ErrorBody),
    NotFound(ErrorBody),
    AlreadyExists(ErrorBody),
    InternalError(ErrorBody),
    CouldNotAuthenticate(ErrorBody),
}

impl ApiErrorDetails for GrpcApiError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            Self::BadRequest(_) => "BadRequest",
            Self::NotFound(_) => "NotFound",
            Self::Unauthorized(_) => "Unauthorized",
            Self::InternalError(_) => "InternalError",
            Self::AlreadyExists(_) => "AlreadyExists",
            Self::CouldNotAuthenticate(_) => "CouldNotAuthenticate",
            Self::LimitExceeded(_) => "LimitExceeded",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            Self::BadRequest(_) => true,
            Self::NotFound(_) => true,
            Self::Unauthorized(_) => true,
            Self::InternalError(_) => false,
            Self::AlreadyExists(_) => true,
            Self::CouldNotAuthenticate(_) => true,
            Self::LimitExceeded(_) => true,
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        match self {
            Self::BadRequest(inner) => inner.cause.take(),
            Self::NotFound(inner) => inner.cause.take(),
            Self::Unauthorized(inner) => inner.cause.take(),
            Self::InternalError(inner) => inner.cause.take(),
            Self::AlreadyExists(inner) => inner.cause.take(),
            Self::CouldNotAuthenticate(inner) => inner.cause.take(),
            Self::LimitExceeded(inner) => inner.cause.take(),
        }
    }
}

impl From<String> for GrpcApiError {
    fn from(value: String) -> Self {
        Self::InternalError(ErrorBody {
            error: value,
            code: api::error_code::INTERNAL_UNKNOWN.to_string(),
            cause: None,
        })
    }
}

impl From<&'static str> for GrpcApiError {
    fn from(value: &'static str) -> Self {
        Self::from(value.to_string())
    }
}

impl From<AuthorizationError> for GrpcApiError {
    fn from(value: AuthorizationError) -> Self {
        Self::Unauthorized(ErrorBody {
            error: value.to_string(),
            code: api::error_code::AUTH_UNAUTHORIZED.to_string(),
            cause: None,
        })
    }
}

impl From<LimitExceededError> for GrpcApiError {
    fn from(value: LimitExceededError) -> Self {
        Self::LimitExceeded(ErrorBody {
            error: value.to_string(),
            code: api::error_code::LIMIT_EXCEEDED.to_string(),
            cause: None,
        })
    }
}

impl From<AuthError> for GrpcApiError {
    fn from(value: AuthError) -> Self {
        let error: String = value.to_string();
        match value {
            AuthError::CouldNotAuthenticate => Self::CouldNotAuthenticate(ErrorBody {
                error,
                code: api::error_code::AUTH_UNAUTHORIZED.to_string(),
                cause: None,
            }),
            _ => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<AccountUsageError> for GrpcApiError {
    fn from(value: AccountUsageError) -> Self {
        let error: String = value.to_string();
        match value {
            AccountUsageError::AccountNotfound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::ACCOUNT_NOT_FOUND.to_string(),
                cause: None,
            }),
            AccountUsageError::Unauthorized(inner) => inner.into(),
            AccountUsageError::LimitExceeded(inner) => inner.into(),
            _ => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<ComponentError> for GrpcApiError {
    fn from(value: ComponentError) -> Self {
        let error: String = value.to_string();
        match value {
            ComponentError::Unauthorized(inner) => inner.into(),
            ComponentError::LimitExceeded(inner) => inner.into(),

            ComponentError::ParentEnvironmentNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::ENVIRONMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            ComponentError::AgentTypeForNameNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::AGENT_TYPE_NOT_FOUND.to_string(),
                cause: None,
            }),
            ComponentError::DeploymentRevisionNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::DEPLOYMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            ComponentError::ComponentNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::COMPONENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            ComponentError::ComponentByNameNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::COMPONENT_NOT_FOUND.to_string(),
                cause: None,
            }),

            _ => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<ComponentResolverError> for GrpcApiError {
    fn from(value: ComponentResolverError) -> Self {
        let error: String = value.to_string();
        match value {
            ComponentResolverError::InvalidComponentReference { .. } => {
                Self::BadRequest(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::INVALID_COMPONENT_REFERENCE.to_string(),
                    cause: None,
                })
            }
            ComponentResolverError::ComponentNotFound => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::COMPONENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            _ => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<DeploymentError> for GrpcApiError {
    fn from(value: DeploymentError) -> Self {
        let error: String = value.to_string();
        match value {
            DeploymentError::ParentEnvironmentNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::ENVIRONMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            DeploymentError::DeploymentNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::DEPLOYMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            DeploymentError::AgentTypeNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::AGENT_TYPE_NOT_FOUND.to_string(),
                cause: None,
            }),

            DeploymentError::Unauthorized(inner) => inner.into(),

            DeploymentError::InternalError(_) => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<DeployedRoutesError> for GrpcApiError {
    fn from(value: DeployedRoutesError) -> Self {
        let error: String = value.to_string();
        match value {
            DeployedRoutesError::NoActiveRoutesForDomain(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::DEPLOYMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            DeployedRoutesError::DeploymentRevisionNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::DEPLOYMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            DeployedRoutesError::InternalError(_) => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<DeployedMcpError> for GrpcApiError {
    fn from(value: DeployedMcpError) -> Self {
        let error: String = value.to_string();

        match value {
            DeployedMcpError::NoActiveMcpForDomain(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::DEPLOYMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            DeployedMcpError::InternalError(_) => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<EnvironmentError> for GrpcApiError {
    fn from(value: EnvironmentError) -> Self {
        let error: String = value.to_string();
        match value {
            EnvironmentError::EnvironmentNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::ENVIRONMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            EnvironmentError::EnvironmentByNameNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::ENVIRONMENT_NOT_FOUND.to_string(),
                cause: None,
            }),
            EnvironmentError::ParentApplicationNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::APPLICATION_NOT_FOUND.to_string(),
                cause: None,
            }),

            EnvironmentError::Unauthorized(inner) => inner.into(),

            EnvironmentError::LimitExceeded(inner) => inner.into(),

            EnvironmentError::InternalError(_) => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
            EnvironmentError::EnvironmentWithNameAlreadyExists => Self::AlreadyExists(ErrorBody {
                error,
                code: api::error_code::ENVIRONMENT_ALREADY_EXISTS.to_string(),
                cause: None,
            }),
            EnvironmentError::ConcurrentModification => Self::AlreadyExists(ErrorBody {
                error,
                code: api::error_code::CONCURRENT_UPDATE.to_string(),
                cause: None,
            }),
        }
    }
}

impl From<EnvironmentStateError> for GrpcApiError {
    fn from(value: EnvironmentStateError) -> Self {
        let error: String = value.to_string();
        match value {
            EnvironmentStateError::InternalError(_) => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
        }
    }
}

impl From<ResourceDefinitionError> for GrpcApiError {
    fn from(value: ResourceDefinitionError) -> Self {
        let error: String = value.to_string();
        match value {
            ResourceDefinitionError::ResourceDefinitionByNameNotFound(_) => {
                Self::NotFound(ErrorBody {
                    error,
                    code: api::error_code::RESOURCE_DEFINITION_NOT_FOUND.to_string(),
                    cause: None,
                })
            }
            ResourceDefinitionError::ResourceDefinitionNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::RESOURCE_DEFINITION_NOT_FOUND.to_string(),
                cause: None,
            }),
            ResourceDefinitionError::ParentEnvironmentNotFound(_) => Self::NotFound(ErrorBody {
                error,
                code: api::error_code::ENVIRONMENT_NOT_FOUND.to_string(),
                cause: None,
            }),

            ResourceDefinitionError::LimitTypeCannotBeChanged => Self::BadRequest(ErrorsBody {
                errors: vec![error],
                code: api::error_code::RESOURCE_LIMIT_TYPE_IMMUTABLE.to_string(),
                cause: None,
            }),

            ResourceDefinitionError::ResourceDefinitionForNameAlreadyExists(_) => {
                Self::AlreadyExists(ErrorBody {
                    error,
                    code: api::error_code::RESOURCE_DEFINITION_ALREADY_EXISTS.to_string(),
                    cause: None,
                })
            }

            ResourceDefinitionError::Unauthorized(inner) => inner.into(),

            ResourceDefinitionError::InternalError(_) => Self::InternalError(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            }),
            ResourceDefinitionError::ConcurrentUpdate => Self::AlreadyExists(ErrorBody {
                error,
                code: api::error_code::CONCURRENT_UPDATE.to_string(),
                cause: None,
            }),
        }
    }
}

impl From<GrpcApiError>
    for golem_api_grpc::proto::golem::registry::v1::registry_service_error::Error
{
    fn from(value: GrpcApiError) -> Self {
        match value {
            GrpcApiError::AlreadyExists(error) => Self::AlreadyExists(error.into()),
            GrpcApiError::BadRequest(error) => Self::BadRequest(error.into()),
            GrpcApiError::CouldNotAuthenticate(error) => Self::CouldNotAuthenticate(error.into()),
            GrpcApiError::InternalError(error) => Self::InternalError(error.into()),
            GrpcApiError::LimitExceeded(error) => Self::LimitExceeded(error.into()),
            GrpcApiError::NotFound(error) => Self::NotFound(error.into()),
            GrpcApiError::Unauthorized(error) => Self::Unauthorized(error.into()),
        }
    }
}

impl From<GrpcApiError> for golem_api_grpc::proto::golem::registry::v1::RegistryServiceError {
    fn from(value: GrpcApiError) -> Self {
        Self {
            error: Some(value.into()),
        }
    }
}
