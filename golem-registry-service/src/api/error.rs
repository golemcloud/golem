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

use crate::services::account::AccountError;
use crate::services::account_usage::error::LimitExceededError;
use crate::services::application::ApplicationError;
use crate::services::auth::AuthError;
use crate::services::component::ComponentError;
use crate::services::deployment::{DeployedRoutesError, DeploymentError, DeploymentWriteError};
use crate::services::domain_registration::DomainRegistrationError;
use crate::services::environment::EnvironmentError;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantError;
use crate::services::environment_share::EnvironmentShareError;
use crate::services::http_api_deployment::HttpApiDeploymentError;
use crate::services::oauth2::OAuth2Error;
use crate::services::plan::PlanError;
use crate::services::plugin_registration::PluginRegistrationError;
use crate::services::reports::ReportsError;
use crate::services::security_scheme::SecuritySchemeError;
use crate::services::token::TokenError;
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::{IntoAnyhow, SafeDisplay};
use golem_service_base::model::auth::AuthorizationError;
use poem_openapi::ApiResponse;
use poem_openapi::payload::Json;

#[derive(ApiResponse, Debug)]
pub enum ApiError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Forbidden Request
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    /// Entity not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    Conflict(Json<ErrorBody>),
    /// Limits of the plan exceeded
    #[oai(status = 422)]
    LimitExceeded(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl ApiError {
    pub fn bad_request(message: String) -> Self {
        Self::BadRequest(Json(ErrorsBody {
            errors: vec![message],
            cause: None,
        }))
    }
}

impl ApiErrorDetails for ApiError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            Self::BadRequest(_) => "BadRequest",
            Self::NotFound(_) => "NotFound",
            Self::Unauthorized(_) => "Unauthorized",
            Self::InternalError(_) => "InternalError",
            Self::Conflict(_) => "Conflict",
            Self::Forbidden(_) => "Forbidden",
            Self::LimitExceeded(_) => "LimitExceeded",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            Self::BadRequest(_) => true,
            Self::NotFound(_) => true,
            Self::Unauthorized(_) => true,
            Self::InternalError(_) => false,
            Self::Forbidden(_) => true,
            Self::Conflict(_) => true,
            Self::LimitExceeded(_) => true,
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        match self {
            Self::BadRequest(inner) => inner.cause.take(),
            Self::NotFound(inner) => inner.cause.take(),
            Self::Unauthorized(inner) => inner.cause.take(),
            Self::InternalError(inner) => inner.cause.take(),
            Self::Forbidden(inner) => inner.cause.take(),
            Self::Conflict(inner) => inner.cause.take(),
            Self::LimitExceeded(inner) => inner.cause.take(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: "Internal error".to_string(),
            cause: Some(value),
        }))
    }
}

impl From<AuthorizationError> for ApiError {
    fn from(value: AuthorizationError) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: value.to_string(),
            cause: None,
        }))
    }
}

impl From<LimitExceededError> for ApiError {
    fn from(value: LimitExceededError) -> Self {
        Self::LimitExceeded(Json(ErrorBody {
            error: value.to_string(),
            cause: None,
        }))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: "Internal Error".to_string(),
            cause: Some(value.into()),
        }))
    }
}

impl From<AuthError> for ApiError {
    fn from(value: AuthError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            AuthError::CouldNotAuthenticate => {
                Self::Unauthorized(Json(ErrorBody { error, cause: None }))
            }
            AuthError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<AccountError> for ApiError {
    fn from(value: AccountError) -> Self {
        let error = value.to_safe_string();
        match value {
            AccountError::Unauthorized(inner) => inner.into(),

            AccountError::AccountNotFound(_)
            | AccountError::AccountByEmailNotFound(_)
            | AccountError::PlanByIdNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            AccountError::EmailAlreadyInUse | AccountError::ConcurrentUpdate => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            AccountError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<ApplicationError> for ApiError {
    fn from(value: ApplicationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ApplicationError::ApplicationWithNameAlreadyExists
            | ApplicationError::ConcurrentModification => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            ApplicationError::ApplicationNotFound(_)
            | ApplicationError::ApplicationByNameNotFound(_)
            | ApplicationError::ParentAccountNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            ApplicationError::Unauthorized(inner) => inner.into(),

            ApplicationError::LimitExceeded(inner) => inner.into(),

            ApplicationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentError> for ApiError {
    fn from(value: EnvironmentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentError::EnvironmentNotFound(_)
            | EnvironmentError::EnvironmentByNameNotFound(_)
            | EnvironmentError::ParentApplicationNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            EnvironmentError::EnvironmentWithNameAlreadyExists
            | EnvironmentError::ConcurrentModification => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            EnvironmentError::Unauthorized(inner) => inner.into(),

            EnvironmentError::LimitExceeded(inner) => inner.into(),

            EnvironmentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<PlanError> for ApiError {
    fn from(value: PlanError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            PlanError::PlanNotFound(_) => Self::NotFound(Json(ErrorBody { error, cause: None })),

            PlanError::Unauthorized(inner) => inner.into(),

            PlanError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<ComponentError> for ApiError {
    fn from(value: ComponentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ComponentError::ComponentProcessingError(_)
            | ComponentError::InitialComponentFileNotFound { .. }
            | ComponentError::InvalidFilePath(_)
            | ComponentError::InvalidComponentName { .. }
            | ComponentError::InvalidOplogProcessorPlugin
            | ComponentError::InvalidPluginScope { .. }
            | ComponentError::MalformedComponentArchive { .. }
            | ComponentError::PluginInstallationNotFound { .. }
            | ComponentError::EnvironmentPluginNotFound(_)
            | ComponentError::ComponentTransformerPluginFailed { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    cause: None,
                }))
            }
            ComponentError::PluginCompositionFailed { cause, .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    cause: Some(cause.context("ComponentError")),
                }))
            }

            ComponentError::ComponentWithNameAlreadyExists(_)
            | ComponentError::ComponentVersionAlreadyExists(_)
            | ComponentError::ConflictingPluginPriority(_)
            | ComponentError::ConflictingEnvironmentPluginGrantId(_)
            | ComponentError::ConcurrentUpdate => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            ComponentError::ParentEnvironmentNotFound(_)
            | ComponentError::DeploymentRevisionNotFound(_)
            | ComponentError::ComponentNotFound(_)
            | ComponentError::ComponentByNameNotFound(_)
            | ComponentError::AgentTypeForNameNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            ComponentError::Unauthorized(inner) => inner.into(),

            ComponentError::LimitExceeded(inner) => inner.into(),

            ComponentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<TokenError> for ApiError {
    fn from(value: TokenError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            TokenError::Unauthorized(inner) => inner.into(),
            TokenError::TokenNotFound(_)
            | TokenError::TokenBySecretNotFound
            | TokenError::ParentAccountNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }
            TokenError::TokenSecretAlreadyExists => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }
            TokenError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<OAuth2Error> for ApiError {
    fn from(value: OAuth2Error) -> Self {
        let error: String = value.to_safe_string();
        match value {
            OAuth2Error::InvalidSession(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                cause: None,
            })),
            OAuth2Error::OAuth2WebflowStateNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }
            OAuth2Error::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentShareError> for ApiError {
    fn from(value: EnvironmentShareError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentShareError::ConcurrentModification
            | EnvironmentShareError::ShareForAccountAlreadyExists => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }
            EnvironmentShareError::EnvironmentShareNotFound(_)
            | EnvironmentShareError::ParentEnvironmentNotFound(_)
            | EnvironmentShareError::EnvironmentShareForGranteeNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }
            EnvironmentShareError::Unauthorized(inner) => inner.into(),
            EnvironmentShareError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<ReportsError> for ApiError {
    fn from(value: ReportsError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ReportsError::Unauthorized(inner) => inner.into(),
            ReportsError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<PluginRegistrationError> for ApiError {
    fn from(value: PluginRegistrationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            PluginRegistrationError::ParentAccountNotFound(_)
            | PluginRegistrationError::PluginRegistrationNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            PluginRegistrationError::RequiredWasmFileMissing
            | PluginRegistrationError::OplogProcessorComponentDoesNotExist => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    cause: None,
                }))
            }

            PluginRegistrationError::PluginNameAndVersionAlreadyExists => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            PluginRegistrationError::Unauthorized(inner) => inner.into(),
            PluginRegistrationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentPluginGrantError> for ApiError {
    fn from(value: EnvironmentPluginGrantError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentPluginGrantError::ParentEnvironmentNotFound(_)
            | EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            EnvironmentPluginGrantError::ReferencedPluginNotFound(_) => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    cause: None,
                }))
            }

            EnvironmentPluginGrantError::GrantForPluginAlreadyExists => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            EnvironmentPluginGrantError::Unauthorized(inner) => inner.into(),
            EnvironmentPluginGrantError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DeploymentWriteError> for ApiError {
    fn from(value: DeploymentWriteError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DeploymentWriteError::ParentEnvironmentNotFound(_)
            | DeploymentWriteError::DeploymentNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            DeploymentWriteError::DeploymentValidationFailed(failed_validations) => {
                // Conflict is probably a better fit
                Self::BadRequest(Json(ErrorsBody {
                    errors: failed_validations
                        .into_iter()
                        .map(|fv| fv.to_safe_string())
                        .collect(),
                    cause: None,
                }))
            }

            DeploymentWriteError::ConcurrentDeployment
            | DeploymentWriteError::NoOpDeployment
            | DeploymentWriteError::VersionAlreadyExists { .. }
            | DeploymentWriteError::DeploymentHashMismatch { .. }
            | DeploymentWriteError::EnvironmentNotYetDeployed => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            DeploymentWriteError::Unauthorized(inner) => inner.into(),
            DeploymentWriteError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DeploymentError> for ApiError {
    fn from(value: DeploymentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DeploymentError::ParentEnvironmentNotFound(_)
            | DeploymentError::DeploymentNotFound(_)
            | DeploymentError::AgentTypeNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            DeploymentError::Unauthorized(inner) => inner.into(),
            DeploymentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DeployedRoutesError> for ApiError {
    fn from(value: DeployedRoutesError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DeployedRoutesError::NoActiveRoutesForDomain(_)
            | DeployedRoutesError::ParentEnvironmentNotFound(_)
            | DeployedRoutesError::DeploymentRevisionNotFound(_)
            | DeployedRoutesError::DomainNotFoundInDeployment(_) => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            DeployedRoutesError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DomainRegistrationError> for ApiError {
    fn from(value: DomainRegistrationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DomainRegistrationError::ParentEnvironmentNotFound(_)
            | DomainRegistrationError::DomainRegistrationNotFound(_)
            | DomainRegistrationError::DomainRegistrationByDomainNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            DomainRegistrationError::DomainCannotBeProvisioned { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    cause: None,
                }))
            }

            DomainRegistrationError::DomainAlreadyExists(_) => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            DomainRegistrationError::Unauthorized(inner) => inner.into(),
            DomainRegistrationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<SecuritySchemeError> for ApiError {
    fn from(value: SecuritySchemeError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            SecuritySchemeError::ParentEnvironmentNotFound(_)
            | SecuritySchemeError::SecuritySchemeNotFound(_)
            | SecuritySchemeError::SecuritySchemeForNameNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            SecuritySchemeError::InvalidRedirectUrl => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                cause: None,
            })),

            SecuritySchemeError::SecuritySchemeWithNameAlreadyExists(_)
            | SecuritySchemeError::ConcurrentUpdateAttempt => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            SecuritySchemeError::Unauthorized(inner) => inner.into(),
            SecuritySchemeError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<HttpApiDeploymentError> for ApiError {
    fn from(value: HttpApiDeploymentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            HttpApiDeploymentError::ParentEnvironmentNotFound(_)
            | HttpApiDeploymentError::DeploymentRevisionNotFound(_)
            | HttpApiDeploymentError::HttpApiDeploymentNotFound(_)
            | HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(_) => {
                Self::NotFound(Json(ErrorBody { error, cause: None }))
            }

            HttpApiDeploymentError::DomainNotRegistered(_)
            | HttpApiDeploymentError::HttpApiDeploymentForDomainAlreadyExists(_)
            | HttpApiDeploymentError::ConcurrentUpdate => {
                Self::Conflict(Json(ErrorBody { error, cause: None }))
            }

            HttpApiDeploymentError::Unauthorized(inner) => inner.into(),
            HttpApiDeploymentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}
