// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::services::account_usage::error::AccountUsageError;
use crate::services::auth::AuthError;
use golem_common::SafeDisplay;
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
            cause: None,
        })
    }
}

impl From<AuthError> for GrpcApiError {
    fn from(value: AuthError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            AuthError::CouldNotAuthenticate => {
                Self::CouldNotAuthenticate(ErrorBody { error, cause: None })
            }
            AuthError::InternalError(inner) => Self::InternalError(ErrorBody {
                error,
                cause: Some(inner.context("AuthError")),
            }),
        }
    }
}

impl From<AccountUsageError> for GrpcApiError {
    fn from(value: AccountUsageError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            AccountUsageError::Unauthorized(authorization_error) => Self::from(authorization_error),
            AccountUsageError::AccountNotfound(_) => {
                Self::NotFound(ErrorBody { error, cause: None })
            }
            AccountUsageError::LimitExceeded { .. } => {
                Self::LimitExceeded(ErrorBody { error, cause: None })
            }
            AccountUsageError::InternalError(inner) => Self::InternalError(ErrorBody {
                error,
                cause: Some(inner.context("AuthError")),
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
