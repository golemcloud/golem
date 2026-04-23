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

use crate::services::account::AccountError;
use crate::services::account_usage::error::LimitExceededError;
use crate::services::agent_secret::AgentSecretError;
use crate::services::application::ApplicationError;
use crate::services::auth::AuthError;
use crate::services::component::ComponentError;
use crate::services::deployment::{DeploymentError, DeploymentWriteError};
use crate::services::domain_registration::DomainRegistrationError;
use crate::services::environment::EnvironmentError;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantError;
use crate::services::environment_share::EnvironmentShareError;
use crate::services::http_api_deployment::HttpApiDeploymentError;
use crate::services::mcp_deployment::McpDeploymentError;
use crate::services::oauth2::OAuth2Error;
use crate::services::plan::PlanError;
use crate::services::plugin_registration::PluginRegistrationError;
use crate::services::reports::ReportsError;
use crate::services::resource_definition::ResourceDefinitionError;
use crate::services::retry_policy::RetryPolicyError;
use crate::services::security_scheme::SecuritySchemeError;
use crate::services::token::TokenError;
use golem_common::base_model::api;
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
    pub fn bad_request(code: &str, message: String) -> Self {
        Self::BadRequest(Json(ErrorsBody {
            errors: vec![message],
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn unauthorized(code: &str, message: String) -> Self {
        Self::Unauthorized(Json(ErrorBody {
            error: message,
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn forbidden(code: &str, message: String) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: message,
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn not_found(code: &str, message: String) -> Self {
        Self::NotFound(Json(ErrorBody {
            error: message,
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn conflict(code: &str, message: String) -> Self {
        Self::Conflict(Json(ErrorBody {
            error: message,
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn limit_exceeded(code: &str, message: String) -> Self {
        Self::LimitExceeded(Json(ErrorBody {
            error: message,
            code: code.to_string(),
            cause: None,
        }))
    }

    pub fn internal(code: &str, message: String, cause: Option<anyhow::Error>) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: message,
            code: code.to_string(),
            cause,
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
            code: api::error_code::INTERNAL_UNKNOWN.to_string(),
            cause: Some(value),
        }))
    }
}

impl From<AuthorizationError> for ApiError {
    fn from(value: AuthorizationError) -> Self {
        Self::Forbidden(Json(ErrorBody {
            error: value.to_string(),
            code: api::error_code::AUTH_FORBIDDEN.to_string(),
            cause: None,
        }))
    }
}

impl From<LimitExceededError> for ApiError {
    fn from(value: LimitExceededError) -> Self {
        Self::LimitExceeded(Json(ErrorBody {
            error: value.to_string(),
            code: api::error_code::LIMIT_EXCEEDED.to_string(),
            cause: None,
        }))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::InternalError(Json(ErrorBody {
            error: "Internal Error".to_string(),
            code: api::error_code::INTERNAL_UNKNOWN.to_string(),
            cause: Some(value.into()),
        }))
    }
}

impl From<AuthError> for ApiError {
    fn from(value: AuthError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            AuthError::CouldNotAuthenticate => Self::Unauthorized(Json(ErrorBody {
                error,
                code: api::error_code::AUTH_UNAUTHORIZED.to_string(),
                cause: None,
            })),
            AuthError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
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
            AccountError::AccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            AccountError::AccountByEmailNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            AccountError::PlanByIdNotFound(_) => {
                Self::not_found(api::error_code::PLAN_NOT_FOUND, error)
            }

            AccountError::EmailAlreadyInUse => {
                Self::conflict(api::error_code::ACCOUNT_EMAIL_ALREADY_IN_USE, error)
            }
            AccountError::ConcurrentUpdate => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            AccountError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<ApplicationError> for ApiError {
    fn from(value: ApplicationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ApplicationError::ApplicationWithNameAlreadyExists => {
                Self::conflict(api::error_code::APPLICATION_ALREADY_EXISTS, error)
            }
            ApplicationError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            ApplicationError::ApplicationNotFound(_) => {
                Self::not_found(api::error_code::APPLICATION_NOT_FOUND, error)
            }
            ApplicationError::ApplicationByNameNotFound(_) => {
                Self::not_found(api::error_code::APPLICATION_NOT_FOUND, error)
            }
            ApplicationError::ParentAccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            ApplicationError::Unauthorized(inner) => inner.into(),
            ApplicationError::LimitExceeded(inner) => inner.into(),
            ApplicationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentError> for ApiError {
    fn from(value: EnvironmentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentError::EnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            EnvironmentError::EnvironmentByNameNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            EnvironmentError::ParentApplicationNotFound(_) => {
                Self::not_found(api::error_code::APPLICATION_NOT_FOUND, error)
            }
            EnvironmentError::EnvironmentWithNameAlreadyExists => {
                Self::conflict(api::error_code::ENVIRONMENT_ALREADY_EXISTS, error)
            }
            EnvironmentError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            EnvironmentError::Unauthorized(inner) => inner.into(),
            EnvironmentError::LimitExceeded(inner) => inner.into(),
            EnvironmentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<PlanError> for ApiError {
    fn from(value: PlanError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            PlanError::PlanNotFound(_) => Self::not_found(api::error_code::PLAN_NOT_FOUND, error),
            PlanError::Unauthorized(inner) => inner.into(),
            PlanError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<ComponentError> for ApiError {
    fn from(value: ComponentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ComponentError::ComponentProcessingError(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                code: api::error_code::COMPONENT_PROCESSING_ERROR.to_string(),
                cause: None,
            })),
            ComponentError::AgentFileNotFoundInArchive { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::INITIAL_COMPONENT_FILE_NOT_FOUND.to_string(),
                    cause: None,
                }))
            }
            ComponentError::InvalidFilePath(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                code: api::error_code::INVALID_FILE_PATH.to_string(),
                cause: None,
            })),
            ComponentError::InvalidOplogProcessorPlugin => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                code: api::error_code::INVALID_OPLOG_PROCESSOR_PLUGIN.to_string(),
                cause: None,
            })),
            ComponentError::InvalidPluginScope { .. } => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                code: api::error_code::INVALID_PLUGIN_SCOPE.to_string(),
                cause: None,
            })),
            ComponentError::MalformedComponentArchive { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::MALFORMED_COMPONENT_ARCHIVE.to_string(),
                    cause: None,
                }))
            }
            ComponentError::PluginInstallationNotFound { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::PLUGIN_INSTALLATION_NOT_FOUND.to_string(),
                    cause: None,
                }))
            }
            ComponentError::AgentConfigDuplicateValue { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::AGENT_CONFIG_DUPLICATE_VALUE.to_string(),
                    cause: None,
                }))
            }
            ComponentError::AgentConfigTypeMismatch { .. } => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                code: api::error_code::AGENT_CONFIG_TYPE_MISMATCH.to_string(),
                cause: None,
            })),
            ComponentError::EnvironmentPluginNotFound(_) => Self::BadRequest(Json(ErrorsBody {
                errors: vec![error],
                code: api::error_code::ENVIRONMENT_PLUGIN_NOT_FOUND.to_string(),
                cause: None,
            })),
            ComponentError::ComponentWithNameAlreadyExists(_) => {
                Self::conflict(api::error_code::COMPONENT_NAME_ALREADY_EXISTS, error)
            }
            ComponentError::ComponentVersionAlreadyExists(_) => {
                Self::conflict(api::error_code::COMPONENT_VERSION_ALREADY_EXISTS, error)
            }
            ComponentError::ConflictingPluginPriority(_) => {
                Self::conflict(api::error_code::PLUGIN_PRIORITY_CONFLICT, error)
            }
            ComponentError::ConflictingEnvironmentPluginGrantId(_) => {
                Self::conflict(api::error_code::ENVIRONMENT_PLUGIN_GRANT_CONFLICT, error)
            }
            ComponentError::AgentConfigNotDeclared { .. } => {
                Self::conflict(api::error_code::AGENT_CONFIG_NOT_DECLARED, error)
            }
            ComponentError::AgentConfigProvidedSecretWhereOnlyLocalAllowed { .. } => {
                Self::conflict(api::error_code::AGENT_CONFIG_SECRET_SCOPE_INVALID, error)
            }
            ComponentError::AgentConfigOldConfigNotValid { .. } => {
                Self::conflict(api::error_code::AGENT_CONFIG_OLD_CONFIG_INVALID, error)
            }
            ComponentError::ConcurrentUpdate => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            ComponentError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            ComponentError::AgentTypeForNameNotFound(_) => {
                Self::not_found(api::error_code::AGENT_TYPE_NOT_FOUND, error)
            }
            ComponentError::DeploymentRevisionNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }
            ComponentError::ComponentNotFound(_) => {
                Self::not_found(api::error_code::COMPONENT_NOT_FOUND, error)
            }
            ComponentError::ComponentByNameNotFound(_) => {
                Self::not_found(api::error_code::COMPONENT_NOT_FOUND, error)
            }
            ComponentError::UndeclaredAgentTypeInProvisionConfig(_) => {
                Self::bad_request(api::error_code::AGENT_TYPE_NOT_DECLARED, error)
            }
            ComponentError::Unauthorized(inner) => inner.into(),

            ComponentError::LimitExceeded(inner) => inner.into(),

            ComponentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
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
            TokenError::TokenNotFound(_) => {
                Self::not_found(api::error_code::TOKEN_NOT_FOUND, error)
            }
            TokenError::TokenBySecretNotFound => {
                Self::not_found(api::error_code::TOKEN_NOT_FOUND, error)
            }
            TokenError::ParentAccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            TokenError::TokenSecretAlreadyExists => {
                Self::conflict(api::error_code::TOKEN_ALREADY_EXISTS, error)
            }
            TokenError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<OAuth2Error> for ApiError {
    fn from(value: OAuth2Error) -> Self {
        let error: String = value.to_safe_string();
        match value {
            OAuth2Error::InvalidSession(_) => {
                Self::bad_request(api::error_code::INVALID_OAUTH_SESSION, error)
            }
            OAuth2Error::OAuth2WebflowStateNotFound(_) => {
                Self::not_found(api::error_code::OAUTH_STATE_NOT_FOUND, error)
            }
            OAuth2Error::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentShareError> for ApiError {
    fn from(value: EnvironmentShareError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentShareError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            EnvironmentShareError::ShareForAccountAlreadyExists => {
                Self::conflict(api::error_code::ENVIRONMENT_SHARE_ALREADY_EXISTS, error)
            }
            EnvironmentShareError::EnvironmentShareNotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_NOT_FOUND, error)
            }
            EnvironmentShareError::EnvironmentShareForGranteeNotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_NOT_FOUND, error)
            }
            EnvironmentShareError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            EnvironmentShareError::Unauthorized(inner) => inner.into(),
            EnvironmentShareError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
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
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<PluginRegistrationError> for ApiError {
    fn from(value: PluginRegistrationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            PluginRegistrationError::ParentAccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            PluginRegistrationError::PluginRegistrationNotFound(_) => {
                Self::not_found(api::error_code::PLUGIN_REGISTRATION_NOT_FOUND, error)
            }

            PluginRegistrationError::OplogProcessorComponentDoesNotExist => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::OPLOG_PROCESSOR_COMPONENT_NOT_FOUND.to_string(),
                    cause: None,
                }))
            }

            PluginRegistrationError::PluginNameAndVersionAlreadyExists => {
                Self::conflict(api::error_code::PLUGIN_REGISTRATION_ALREADY_EXISTS, error)
            }

            PluginRegistrationError::Unauthorized(inner) => inner.into(),
            PluginRegistrationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentPluginGrantError> for ApiError {
    fn from(value: EnvironmentPluginGrantError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            EnvironmentPluginGrantError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_NOT_FOUND, error)
            }

            EnvironmentPluginGrantError::ReferencedPluginNotFound(_) => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::REFERENCED_PLUGIN_NOT_FOUND.to_string(),
                    cause: None,
                }))
            }

            EnvironmentPluginGrantError::GrantForPluginAlreadyExists => Self::conflict(
                api::error_code::ENVIRONMENT_PLUGIN_GRANT_ALREADY_EXISTS,
                error,
            ),

            EnvironmentPluginGrantError::CannotDeleteBuiltinPluginGrant(_) => Self::forbidden(
                api::error_code::BUILTIN_PLUGIN_GRANT_CANNOT_BE_DELETED,
                error,
            ),

            EnvironmentPluginGrantError::Unauthorized(inner) => inner.into(),
            EnvironmentPluginGrantError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DeploymentWriteError> for ApiError {
    fn from(value: DeploymentWriteError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DeploymentWriteError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            DeploymentWriteError::DeploymentNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }

            DeploymentWriteError::DeploymentValidationFailed(failed_validations) => {
                // Conflict is probably a better fit
                Self::BadRequest(Json(ErrorsBody {
                    errors: failed_validations
                        .into_iter()
                        .map(|fv| fv.to_safe_string())
                        .collect(),
                    code: api::error_code::DEPLOYMENT_VALIDATION_FAILED.to_string(),
                    cause: None,
                }))
            }

            DeploymentWriteError::ConcurrentDeployment => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            DeploymentWriteError::NoOpDeployment => {
                Self::conflict(api::error_code::DEPLOYMENT_NOOP, error)
            }
            DeploymentWriteError::VersionAlreadyExists { .. } => {
                Self::conflict(api::error_code::DEPLOYMENT_VERSION_ALREADY_EXISTS, error)
            }
            DeploymentWriteError::DeploymentHashMismatch { .. } => {
                Self::conflict(api::error_code::DEPLOYMENT_HASH_MISMATCH, error)
            }
            DeploymentWriteError::EnvironmentNotYetDeployed => {
                Self::conflict(api::error_code::ENVIRONMENT_NOT_DEPLOYED, error)
            }

            DeploymentWriteError::Unauthorized(inner) => inner.into(),
            DeploymentWriteError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DeploymentError> for ApiError {
    fn from(value: DeploymentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DeploymentError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            DeploymentError::DeploymentNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }
            DeploymentError::AgentTypeNotFound(_) => {
                Self::not_found(api::error_code::AGENT_TYPE_NOT_FOUND, error)
            }

            DeploymentError::Unauthorized(inner) => inner.into(),
            DeploymentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<DomainRegistrationError> for ApiError {
    fn from(value: DomainRegistrationError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            DomainRegistrationError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            DomainRegistrationError::DomainRegistrationNotFound(_) => {
                Self::not_found(api::error_code::DOMAIN_REGISTRATION_NOT_FOUND, error)
            }
            DomainRegistrationError::DomainRegistrationByDomainNotFound(_) => {
                Self::not_found(api::error_code::DOMAIN_REGISTRATION_NOT_FOUND, error)
            }

            DomainRegistrationError::DomainCannotBeProvisioned { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::DOMAIN_CANNOT_BE_PROVISIONED.to_string(),
                    cause: None,
                }))
            }

            DomainRegistrationError::DomainAlreadyExists(_) => {
                Self::conflict(api::error_code::DOMAIN_ALREADY_EXISTS, error)
            }

            DomainRegistrationError::DomainNotValidForHttpApi(_) => {
                Self::bad_request(api::error_code::DOMAIN_NOT_VALID_FOR_HTTP_API, error)
            }

            DomainRegistrationError::DomainNotValidForMcp(_) => {
                Self::bad_request(api::error_code::DOMAIN_NOT_VALID_FOR_MCP, error)
            }

            DomainRegistrationError::Unauthorized(inner) => inner.into(),
            DomainRegistrationError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<SecuritySchemeError> for ApiError {
    fn from(value: SecuritySchemeError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            SecuritySchemeError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            SecuritySchemeError::SecuritySchemeNotFound(_) => {
                Self::not_found(api::error_code::SECURITY_SCHEME_NOT_FOUND, error)
            }
            SecuritySchemeError::SecuritySchemeForNameNotFound(_) => {
                Self::not_found(api::error_code::SECURITY_SCHEME_NOT_FOUND, error)
            }

            SecuritySchemeError::InvalidRedirectUrl => {
                Self::bad_request(api::error_code::INVALID_REDIRECT_URL, error)
            }
            SecuritySchemeError::InvalidCustomProviderIssuerUrl(_) => {
                Self::bad_request(api::error_code::INVALID_CUSTOM_PROVIDER_ISSUER_URL, error)
            }

            SecuritySchemeError::SecuritySchemeWithNameAlreadyExists(_) => {
                Self::conflict(api::error_code::SECURITY_SCHEME_ALREADY_EXISTS, error)
            }
            SecuritySchemeError::ConcurrentUpdateAttempt => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }

            SecuritySchemeError::Unauthorized(inner) => inner.into(),
            SecuritySchemeError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<HttpApiDeploymentError> for ApiError {
    fn from(value: HttpApiDeploymentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            HttpApiDeploymentError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            HttpApiDeploymentError::DeploymentRevisionNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }
            HttpApiDeploymentError::HttpApiDeploymentNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }
            HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }

            HttpApiDeploymentError::DomainNotRegistered(_) => Self::Conflict(Json(ErrorBody {
                error,
                code: api::error_code::DOMAIN_NOT_REGISTERED.to_string(),
                cause: None,
            })),

            HttpApiDeploymentError::DomainNotValidForHttpApi(_) => {
                Self::bad_request(api::error_code::DOMAIN_NOT_VALID_FOR_HTTP_API, error)
            }

            HttpApiDeploymentError::HttpApiDeploymentForDomainAlreadyExists(_) => {
                Self::conflict(api::error_code::HTTP_API_DEPLOYMENT_ALREADY_EXISTS, error)
            }
            HttpApiDeploymentError::ConcurrentUpdate => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }

            HttpApiDeploymentError::Unauthorized(inner) => inner.into(),
            HttpApiDeploymentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<McpDeploymentError> for ApiError {
    fn from(value: McpDeploymentError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            McpDeploymentError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            McpDeploymentError::DeploymentRevisionNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }
            McpDeploymentError::McpDeploymentNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }
            McpDeploymentError::McpDeploymentByDomainNotFound(_) => {
                Self::not_found(api::error_code::DEPLOYMENT_NOT_FOUND, error)
            }

            McpDeploymentError::DomainNotRegistered(_) => Self::Conflict(Json(ErrorBody {
                error,
                code: api::error_code::DOMAIN_NOT_REGISTERED.to_string(),
                cause: None,
            })),

            McpDeploymentError::DomainNotValidForMcp(_) => {
                Self::bad_request(api::error_code::DOMAIN_NOT_VALID_FOR_MCP, error)
            }

            McpDeploymentError::McpDeploymentForDomainAlreadyExists(_) => {
                Self::conflict(api::error_code::MCP_DEPLOYMENT_ALREADY_EXISTS, error)
            }
            McpDeploymentError::ConcurrentUpdate => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }

            McpDeploymentError::Unauthorized(inner) => inner.into(),
            McpDeploymentError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<AgentSecretError> for ApiError {
    fn from(value: AgentSecretError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            AgentSecretError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            AgentSecretError::AgentSecretForPathAlreadyExists { .. } => {
                Self::conflict(api::error_code::AGENT_SECRET_ALREADY_EXISTS, error)
            }
            AgentSecretError::AgentSecretValueDoesNotMatchType { .. } => {
                Self::bad_request(api::error_code::AGENT_SECRET_VALUE_TYPE_MISMATCH, error)
            }
            AgentSecretError::AgentSecretNotFound(_) => {
                Self::not_found(api::error_code::AGENT_SECRET_NOT_FOUND, error)
            }
            AgentSecretError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            AgentSecretError::Unauthorized(inner) => inner.into(),
            AgentSecretError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<RetryPolicyError> for ApiError {
    fn from(value: RetryPolicyError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            RetryPolicyError::InvalidPredicateJson(_) => {
                Self::bad_request(api::error_code::RETRY_POLICY_INVALID_PREDICATE_JSON, error)
            }
            RetryPolicyError::InvalidPolicyJson(_) => {
                Self::bad_request(api::error_code::RETRY_POLICY_INVALID_POLICY_JSON, error)
            }
            RetryPolicyError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            RetryPolicyError::RetryPolicyForNameAlreadyExists { .. } => {
                Self::conflict(api::error_code::RETRY_POLICY_ALREADY_EXISTS, error)
            }
            RetryPolicyError::RetryPolicyNotFound(_) => {
                Self::not_found(api::error_code::RETRY_POLICY_NOT_FOUND, error)
            }
            RetryPolicyError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            RetryPolicyError::Unauthorized(inner) => inner.into(),
            RetryPolicyError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                error,
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<ResourceDefinitionError> for ApiError {
    fn from(value: ResourceDefinitionError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            ResourceDefinitionError::ConcurrentUpdate => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            ResourceDefinitionError::LimitTypeCannotBeChanged => {
                Self::bad_request(api::error_code::RESOURCE_LIMIT_TYPE_IMMUTABLE, error)
            }
            ResourceDefinitionError::ResourceDefinitionForNameAlreadyExists(_) => {
                Self::conflict(api::error_code::RESOURCE_DEFINITION_ALREADY_EXISTS, error)
            }
            ResourceDefinitionError::ResourceDefinitionNotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_DEFINITION_NOT_FOUND, error)
            }
            ResourceDefinitionError::ResourceDefinitionByNameNotFound(_) => {
                Self::not_found(api::error_code::RESOURCE_DEFINITION_NOT_FOUND, error)
            }
            ResourceDefinitionError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            ResourceDefinitionError::Unauthorized(inner) => inner.into(),
            ResourceDefinitionError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}
