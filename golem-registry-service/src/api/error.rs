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
use crate::services::account_resource_override::AccountResourceOverrideError;
use crate::services::account_usage::error::AccountUsageError;
use crate::services::account_usage::error::LimitExceededError;
use crate::services::agent_secret::AgentSecretError;
use crate::services::application::ApplicationError;
use crate::services::auth::AuthError;
use crate::services::card::CardError;
use crate::services::component::ComponentError;
use crate::services::deployment::{DeployValidationError, DeploymentError, DeploymentWriteError};
use crate::services::domain_registration::DomainRegistrationError;
use crate::services::environment::EnvironmentError;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantError;
use crate::services::environment_tool_grant::{
    EnvironmentToolGrantError, EnvironmentToolValidationError,
};
use crate::services::http_api_deployment::HttpApiDeploymentError;
use crate::services::mcp_deployment::McpDeploymentError;
use crate::services::oauth2::OAuth2Error;
use crate::services::permission_share::PermissionShareError;
use crate::services::plan::PlanError;
use crate::services::plugin_registration::PluginRegistrationError;
use crate::services::reports::ReportsError;
use crate::services::resource_definition::ResourceDefinitionError;
use crate::services::retry_policy::RetryPolicyError;
use crate::services::security_scheme::SecuritySchemeError;
use crate::services::token::TokenError;
use crate::services::tool_release::ToolReleaseError;
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

fn deployment_validation_subcode(error: &DeployValidationError) -> &'static str {
    match error {
        DeployValidationError::AgentSecretNotCompatibleWithEnvironmentSecret { .. } => {
            api::error_code::deployment_validation::AGENT_SECRET_NOT_COMPATIBLE
        }
        DeployValidationError::AgentSecretTypeConflict { .. } => {
            api::error_code::deployment_validation::AGENT_SECRET_TYPE_CONFLICT
        }
        DeployValidationError::AgentSecretDefaultTypeMismatch { .. } => {
            api::error_code::deployment_validation::AGENT_SECRET_DEFAULT_TYPE_MISMATCH
        }
        DeployValidationError::AgentSecretInvalidConfigType { .. } => {
            api::error_code::deployment_validation::AGENT_SECRET_INVALID_CONFIG_TYPE
        }
        DeployValidationError::NoSecuritySchemeConfigured(_) => {
            api::error_code::deployment_validation::NO_SECURITY_SCHEME_CONFIGURED
        }
        DeployValidationError::McpDeploymentConflictingSecuritySchemes { .. } => {
            api::error_code::deployment_validation::MCP_CONFLICTING_SECURITY_SCHEMES
        }
        DeployValidationError::McpDeploymentUnknownSecurityScheme { .. } => {
            api::error_code::deployment_validation::MCP_UNKNOWN_SECURITY_SCHEME
        }
        DeployValidationError::SecurityOverrideDisabled => {
            api::error_code::deployment_validation::SECURITY_OVERRIDE_DISABLED
        }
        DeployValidationError::HttpApiDefinitionInvalidPathPattern(_) => {
            api::error_code::deployment_validation::HTTP_API_INVALID_PATH_PATTERN
        }
        DeployValidationError::InvalidHttpCorsBindingExpr(_) => {
            api::error_code::deployment_validation::INVALID_HTTP_CORS_BINDING_EXPR
        }
        DeployValidationError::HttpApiDeploymentAgentMethodInvalid { .. } => {
            api::error_code::deployment_validation::HTTP_API_AGENT_METHOD_INVALID
        }
        DeployValidationError::HttpApiDeploymentAgentConstructorInvalid { .. } => {
            api::error_code::deployment_validation::HTTP_API_AGENT_CONSTRUCTOR_INVALID
        }
        DeployValidationError::HttpApiDeploymentInvalidRoute { .. } => {
            api::error_code::deployment_validation::HTTP_API_INVALID_ROUTE
        }
        DeployValidationError::RouteIsAmbiguous { .. } => {
            api::error_code::deployment_validation::ROUTE_IS_AMBIGUOUS
        }
        DeployValidationError::InvalidHttpMethod { .. } => {
            api::error_code::deployment_validation::INVALID_HTTP_METHOD
        }
        DeployValidationError::HttpApiDeploymentMissingAgentType { .. } => {
            api::error_code::deployment_validation::HTTP_API_MISSING_AGENT_TYPE
        }
        DeployValidationError::McpDeploymentMissingAgentType { .. } => {
            api::error_code::deployment_validation::MCP_MISSING_AGENT_TYPE
        }
        DeployValidationError::ComponentNotFound(_) => {
            api::error_code::deployment_validation::COMPONENT_NOT_FOUND
        }
        DeployValidationError::HttpApiDeploymentMultipleDeploymentsForAgentType { .. } => {
            api::error_code::deployment_validation::HTTP_API_MULTIPLE_DEPLOYMENTS_FOR_AGENT_TYPE
        }
        DeployValidationError::HttpApiDeploymentAgentTypeMissingHttpMount { .. } => {
            api::error_code::deployment_validation::HTTP_API_AGENT_TYPE_MISSING_HTTP_MOUNT
        }
        DeployValidationError::HttpApiDeploymentInvalidAgentWebhookSegmentType { .. } => {
            api::error_code::deployment_validation::HTTP_API_INVALID_AGENT_WEBHOOK_SEGMENT_TYPE
        }
        DeployValidationError::AmbiguousAgentTypeName(_) => {
            api::error_code::deployment_validation::AMBIGUOUS_AGENT_TYPE_NAME
        }
        DeployValidationError::ConflictingAgentTypeNames { .. } => {
            api::error_code::deployment_validation::CONFLICTING_AGENT_TYPE_NAMES
        }
        DeployValidationError::ConflictingResourceDefinitions { .. } => {
            api::error_code::deployment_validation::CONFLICTING_RESOURCE_DEFINITIONS
        }
        DeployValidationError::ConflictingRetryPolicyDefaults { .. } => {
            api::error_code::deployment_validation::CONFLICTING_RETRY_POLICY_DEFAULTS
        }
        DeployValidationError::ResetOverrideRequiresCompatibilityCheckDisabled => {
            api::error_code::deployment_validation::RESET_OVERRIDE_REQUIRES_COMPATIBILITY_CHECK_DISABLED
        }
        DeployValidationError::ToolUnsupportedGuestExport { .. } => {
            api::error_code::deployment_validation::TOOL_UNSUPPORTED_GUEST_EXPORT
        }
        DeployValidationError::ToolDefinitionNameMismatch { .. } => {
            api::error_code::deployment_validation::TOOL_DEFINITION_NAME_MISMATCH
        }
        DeployValidationError::InvalidTool { .. }
        | DeployValidationError::ToolMetadataSerialization { .. } => {
            api::error_code::deployment_validation::INVALID_TOOL
        }
        DeployValidationError::DuplicateToolImplementation { .. } => {
            api::error_code::deployment_validation::DUPLICATE_TOOL_IMPLEMENTATION
        }
        DeployValidationError::ToolSourceCollision { .. } => {
            api::error_code::deployment_validation::TOOL_SOURCE_COLLISION
        }
        DeployValidationError::RemoteToolUnavailable { .. } => {
            api::error_code::deployment_validation::REMOTE_TOOL_UNAVAILABLE
        }
        DeployValidationError::RemoteToolNameMismatch { .. }
        | DeployValidationError::RemoteToolDefinitionNameMismatch { .. }
        | DeployValidationError::RemoteToolVersionMismatch { .. } => {
            api::error_code::deployment_validation::REMOTE_TOOL_IDENTITY_MISMATCH
        }
        DeployValidationError::RemoteToolUnsupportedMetadataVersion { .. } => {
            api::error_code::deployment_validation::REMOTE_TOOL_UNSUPPORTED_METADATA_VERSION
        }
        DeployValidationError::RemoteToolMetadataDigestMismatch { .. } => {
            api::error_code::deployment_validation::REMOTE_TOOL_METADATA_DIGEST_MISMATCH
        }
        DeployValidationError::InvalidRemoteTool { .. } => {
            api::error_code::deployment_validation::INVALID_REMOTE_TOOL
        }
        DeployValidationError::RemoteToolBindingUnknownAgent { .. } => {
            api::error_code::deployment_validation::REMOTE_TOOL_BINDING_UNKNOWN_AGENT
        }
        DeployValidationError::ToolBindingUnknownAgent { .. } => {
            api::error_code::deployment_validation::TOOL_BINDING_UNKNOWN_AGENT
        }
        DeployValidationError::ToolBindingVersionMismatch { .. } => {
            api::error_code::deployment_validation::TOOL_BINDING_VERSION_MISMATCH
        }
        DeployValidationError::ToolBindingAccountMismatch { .. } => {
            api::error_code::deployment_validation::TOOL_BINDING_ACCOUNT_MISMATCH
        }
        DeployValidationError::ToolBindingParametersMustBeObject { .. } => {
            api::error_code::deployment_validation::TOOL_BINDING_PARAMETERS_MUST_BE_OBJECT
        }
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

impl From<AccountResourceOverrideError> for ApiError {
    fn from(value: AccountResourceOverrideError) -> Self {
        let error = value.to_safe_string();
        match value {
            AccountResourceOverrideError::NotUserConfigurable(_) => Self::bad_request(
                api::error_code::RESOURCE_OVERRIDE_NOT_USER_CONFIGURABLE,
                error,
            ),
            AccountResourceOverrideError::ExceedsPlanCeiling(_, _) => {
                Self::limit_exceeded(api::error_code::LIMIT_EXCEEDED, error)
            }
            AccountResourceOverrideError::BelowPlanDefault(_, _) => {
                Self::bad_request(api::error_code::LIMIT_EXCEEDED, error)
            }
            AccountResourceOverrideError::ExpiryRequiresAdmin => {
                Self::forbidden(api::error_code::AUTH_FORBIDDEN, error)
            }
            AccountResourceOverrideError::AccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            AccountResourceOverrideError::Unauthorized(inner) => inner.into(),
            AccountResourceOverrideError::InternalError(_) => {
                Self::InternalError(Json(ErrorBody {
                    error,
                    code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                    cause: Some(value.into_anyhow()),
                }))
            }
        }
    }
}

impl From<AccountUsageError> for ApiError {
    fn from(value: AccountUsageError) -> Self {
        let error = value.to_safe_string();
        match value {
            AccountUsageError::LimitExceeded(inner) => inner.into(),
            AccountUsageError::ComponentTooLarge(_) => {
                Self::bad_request(api::error_code::LIMIT_EXCEEDED, error)
            }
            AccountUsageError::AccountNotfound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            AccountUsageError::Unauthorized(inner) => inner.into(),
            AccountUsageError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<CardError> for ApiError {
    fn from(value: CardError) -> Self {
        let error = value.to_safe_string();
        match value {
            CardError::CardNotFound(_) => Self::not_found(api::error_code::CARD_NOT_FOUND, error),
            CardError::AccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            CardError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            CardError::RuntimeCardConflict(_) | CardError::RuntimeCardRevoked(_) => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            CardError::RuntimeCardCannotBeSystemCard => {
                Self::bad_request(api::error_code::INVALID_RUNTIME_CARD, error)
            }
            CardError::CannotRevokeSystemCard
            | CardError::CannotRevokePermissionShareCard
            | CardError::CannotRevokeEnvironmentDefaultCard
            | CardError::CardOwnerNotFound(_)
            | CardError::Unauthorized(_) => Self::forbidden(api::error_code::AUTH_FORBIDDEN, error),
            CardError::InternalError(_) => Self::InternalError(Json(ErrorBody {
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
            EnvironmentError::MutableToolGrantsInVersionCheckedEnvironment => {
                Self::bad_request(api::error_code::ENVIRONMENT_TOOL_GRANT_CONFLICT, error)
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
            ComponentError::AgentConfigPathSegmentContainsDot { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                    code: api::error_code::AGENT_CONFIG_NOT_DECLARED.to_string(),
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
            ComponentError::ResetOverrideRequiresCompatibilityCheckDisabled => Self::conflict(
                api::error_code::RESET_OVERRIDE_REQUIRES_COMPATIBILITY_CHECK_DISABLED,
                error,
            ),
            ComponentError::ConcurrentUpdate => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            ComponentError::ComponentSourceInUse(_) => {
                Self::conflict(api::error_code::COMPONENT_IN_USE, error)
            }
            ComponentError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            ComponentError::AgentTypeForNameNotFound(_) => {
                Self::not_found(api::error_code::AGENT_TYPE_NOT_FOUND, error)
            }
            ComponentError::DuplicateAgentTypeName(_) => {
                Self::bad_request(api::error_code::DUPLICATE_AGENT_TYPE_NAME, error)
            }
            ComponentError::DuplicateToolName(_) => {
                Self::bad_request(api::error_code::DUPLICATE_TOOL_NAME, error)
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
            ComponentError::MissingAgentTypeProvisionConfig(_) => {
                Self::bad_request(api::error_code::AGENT_TYPE_NOT_DECLARED, error)
            }
            ComponentError::MissingToolName
            | ComponentError::InvalidToolName { .. }
            | ComponentError::InvalidTool { .. }
            | ComponentError::ToolDefinitionNameMismatch { .. } => {
                Self::bad_request(api::error_code::INVALID_TOOL_METADATA, error)
            }
            ComponentError::ToolsRequireSupportedGuestExport { .. } => {
                Self::bad_request(api::error_code::TOOL_GUEST_EXPORT_INVALID, error)
            }
            ComponentError::UndeclaredToolInDeploymentConfig(_) => {
                Self::bad_request(api::error_code::TOOL_NOT_DECLARED, error)
            }
            ComponentError::MissingToolDeploymentConfig(_) => {
                Self::bad_request(api::error_code::TOOL_DEPLOYMENT_CONFIG_MISSING, error)
            }
            ComponentError::ToolFileNotFoundInArchive { .. } => {
                Self::bad_request(api::error_code::INITIAL_COMPONENT_FILE_NOT_FOUND, error)
            }
            ComponentError::ConflictingToolFileTarget { .. } => {
                Self::bad_request(api::error_code::INVALID_COMPONENT_FILE_PATH, error)
            }
            ComponentError::NewAgentTypeMissingInitialPermissions(_) => Self::bad_request(
                api::error_code::NEW_AGENT_TYPE_MISSING_INITIAL_PERMISSIONS,
                error,
            ),
            ComponentError::InvalidAgentInitialPermissionCard { .. } => Self::bad_request(
                api::error_code::INVALID_AGENT_INITIAL_PERMISSION_CARD,
                error,
            ),
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
            OAuth2Error::InvalidRedirectDomain(_) => {
                Self::bad_request(api::error_code::INVALID_REDIRECT_URL, error)
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

impl From<PermissionShareError> for ApiError {
    fn from(value: PermissionShareError) -> Self {
        let error: String = value.to_safe_string();
        match value {
            PermissionShareError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            PermissionShareError::PermissionShareAlreadyExists => {
                Self::conflict(api::error_code::PERMISSION_SHARE_ALREADY_EXISTS, error)
            }
            PermissionShareError::PermissionShareNotFound(_) => {
                Self::not_found(api::error_code::PERMISSION_SHARE_NOT_FOUND, error)
            }
            PermissionShareError::PermissionShareByNameNotFound(_) => {
                Self::not_found(api::error_code::PERMISSION_SHARE_NOT_FOUND, error)
            }
            PermissionShareError::TargetAccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            PermissionShareError::InvalidGrant { .. }
            | PermissionShareError::InvalidRecipient { .. } => {
                Self::bad_request(api::error_code::INVALID_PERMISSION_SHARE_GRANT, error)
            }
            PermissionShareError::GrantNotDelegable(_) => {
                Self::forbidden(api::error_code::AUTH_FORBIDDEN, error)
            }
            PermissionShareError::Unauthorized(inner) => inner.into(),
            PermissionShareError::InternalError(_) => Self::InternalError(Json(ErrorBody {
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

impl From<ToolReleaseError> for ApiError {
    fn from(value: ToolReleaseError) -> Self {
        let error = value.to_safe_string();
        match value {
            ToolReleaseError::ToolReleaseNotFound(_) => {
                Self::not_found(api::error_code::TOOL_RELEASE_NOT_FOUND, error)
            }
            ToolReleaseError::ReferencedToolReleaseNotFound => {
                Self::not_found(api::error_code::REFERENCED_TOOL_RELEASE_NOT_FOUND, error)
            }
            ToolReleaseError::ParentAccountNotFound(_) => {
                Self::not_found(api::error_code::ACCOUNT_NOT_FOUND, error)
            }
            ToolReleaseError::PublicationToolNotFound(_) => {
                Self::bad_request(api::error_code::TOOL_PUBLICATION_TOOL_NOT_FOUND, error)
            }
            ToolReleaseError::DuplicatePublication(_) => {
                Self::bad_request(api::error_code::DUPLICATE_TOOL_PUBLICATION, error)
            }
            ToolReleaseError::PublicationOwnerMismatch(_) => {
                Self::bad_request(api::error_code::TOOL_PUBLICATION_OWNER_MISMATCH, error)
            }
            ToolReleaseError::PublicationHostSource(_) => Self::bad_request(
                api::error_code::TOOL_PUBLICATION_HOST_SOURCE_NOT_SUPPORTED,
                error,
            ),
            ToolReleaseError::ImmutableReleaseConflict => {
                Self::conflict(api::error_code::TOOL_RELEASE_IMMUTABLE_CONFLICT, error)
            }
            ToolReleaseError::DePublishedReleaseRequiresExplicitRestore
            | ToolReleaseError::ToolReleaseNotPublished
            | ToolReleaseError::ToolReleaseNotDePublished => {
                Self::conflict(api::error_code::TOOL_RELEASE_LIFECYCLE_CONFLICT, error)
            }
            ToolReleaseError::ProtectedToolRelease => {
                Self::forbidden(api::error_code::AUTH_FORBIDDEN, error)
            }
            ToolReleaseError::Unauthorized(inner) => inner.into(),
            ToolReleaseError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentToolGrantError> for ApiError {
    fn from(value: EnvironmentToolGrantError) -> Self {
        let error = value.to_safe_string();
        match value {
            EnvironmentToolGrantError::ParentEnvironmentNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_NOT_FOUND, error)
            }
            EnvironmentToolGrantError::EnvironmentToolGrantNotFound(_) => {
                Self::not_found(api::error_code::ENVIRONMENT_TOOL_GRANT_NOT_FOUND, error)
            }
            EnvironmentToolGrantError::ReferencedToolReleaseNotFound => {
                Self::not_found(api::error_code::REFERENCED_TOOL_RELEASE_NOT_FOUND, error)
            }
            EnvironmentToolGrantError::GrantAlreadyExists => Self::conflict(
                api::error_code::ENVIRONMENT_TOOL_GRANT_ALREADY_EXISTS,
                error,
            ),
            EnvironmentToolGrantError::GrantNotDeleted(_)
            | EnvironmentToolGrantError::AdministratorManagedToolGrant(_) => {
                Self::conflict(api::error_code::ENVIRONMENT_TOOL_GRANT_CONFLICT, error)
            }
            EnvironmentToolGrantError::ConcurrentModification => {
                Self::conflict(api::error_code::CONCURRENT_UPDATE, error)
            }
            EnvironmentToolGrantError::ProtectedToolGrant(_) => {
                Self::forbidden(api::error_code::AUTH_FORBIDDEN, error)
            }
            EnvironmentToolGrantError::Unauthorized(inner) => inner.into(),
            EnvironmentToolGrantError::InternalError(_) => Self::InternalError(Json(ErrorBody {
                error,
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                cause: Some(value.into_anyhow()),
            })),
        }
    }
}

impl From<EnvironmentToolValidationError> for ApiError {
    fn from(value: EnvironmentToolValidationError) -> Self {
        match value {
            EnvironmentToolValidationError::Grant(error) => error.into(),
            EnvironmentToolValidationError::Publication(error) => error.into(),
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
                Self::BadRequest(Json(ErrorsBody {
                    errors: failed_validations
                        .into_iter()
                        .map(|fv| {
                            format!(
                                "{}: {}",
                                deployment_validation_subcode(&fv),
                                fv.to_safe_string()
                            )
                        })
                        .collect(),
                    code: api::error_code::deployment_validation::FAILED.to_string(),
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
            DeploymentWriteError::ToolReleaseImmutableConflict => {
                Self::conflict(api::error_code::TOOL_RELEASE_IMMUTABLE_CONFLICT, error)
            }
            DeploymentWriteError::ToolReleaseDePublishedConflict => {
                Self::conflict(api::error_code::TOOL_RELEASE_LIFECYCLE_CONFLICT, error)
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
            DeploymentError::ToolNotFound(_) => {
                Self::not_found(api::error_code::TOOL_NOT_FOUND, error)
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

            DomainRegistrationError::DomainNotValidForHttpApi { .. } => {
                Self::bad_request(api::error_code::DOMAIN_NOT_VALID_FOR_HTTP_API, error)
            }

            DomainRegistrationError::DomainNotValidForMcp { .. } => {
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

            HttpApiDeploymentError::DomainNotValidForHttpApi { .. } => {
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

            McpDeploymentError::DomainNotValidForMcp { .. } => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::model::card::CardRepoError;
    use crate::services::component::ComponentError;
    use golem_common::base_model::agent_secret::CanonicalAgentSecretPath;
    use golem_common::base_model::quota::ResourceName;
    use test_r::test;

    #[test]
    fn resource_override_errors_use_distinct_http_statuses() {
        assert!(matches!(
            ApiError::from(AccountResourceOverrideError::NotUserConfigurable(
                "Storage limit",
            )),
            ApiError::BadRequest(_)
        ));
        assert!(matches!(
            ApiError::from(AccountResourceOverrideError::ExceedsPlanCeiling(
                "Storage limit",
                10,
            )),
            ApiError::LimitExceeded(_)
        ));
        assert!(matches!(
            ApiError::from(AccountResourceOverrideError::ExpiryRequiresAdmin),
            ApiError::Forbidden(_)
        ));
        assert!(matches!(
            ApiError::from(AccountResourceOverrideError::AccountNotFound(
                golem_common::model::account::AccountId::new()
            )),
            ApiError::NotFound(_)
        ));
    }

    fn bad_request_from_validations(errors: Vec<DeployValidationError>) -> ErrorsBody {
        let api_error = ApiError::from(DeploymentWriteError::DeploymentValidationFailed(errors));

        match api_error {
            ApiError::BadRequest(body) => body.0,
            other => panic!("Expected BadRequest, got: {other:?}"),
        }
    }

    #[test]
    fn deployment_validation_single_error_uses_specific_code() {
        let body =
            bad_request_from_validations(vec![DeployValidationError::AgentSecretTypeConflict {
                path: CanonicalAgentSecretPath(vec!["apiKey".to_string()]),
            }]);

        assert_eq!(body.code, api::error_code::deployment_validation::FAILED);
        assert_eq!(body.errors.len(), 1);
        assert!(body.errors[0].starts_with(&format!(
            "{}: ",
            api::error_code::deployment_validation::AGENT_SECRET_TYPE_CONFLICT
        )));
    }

    #[test]
    fn deployment_validation_homogeneous_multi_error_uses_failed_top_level_code() {
        let body = bad_request_from_validations(vec![
            DeployValidationError::AgentSecretTypeConflict {
                path: CanonicalAgentSecretPath(vec!["apiKey".to_string()]),
            },
            DeployValidationError::AgentSecretTypeConflict {
                path: CanonicalAgentSecretPath(vec!["dbPassword".to_string()]),
            },
        ]);

        assert_eq!(body.code, api::error_code::deployment_validation::FAILED);
        assert_eq!(body.errors.len(), 2);
        assert!(body.errors.iter().all(|error| {
            error.starts_with(&format!(
                "{}: ",
                api::error_code::deployment_validation::AGENT_SECRET_TYPE_CONFLICT
            ))
        }));
    }

    #[test]
    fn deployment_validation_mixed_multi_error_uses_failed_top_level_code() {
        let body = bad_request_from_validations(vec![
            DeployValidationError::AgentSecretTypeConflict {
                path: CanonicalAgentSecretPath(vec!["apiKey".to_string()]),
            },
            DeployValidationError::ConflictingResourceDefinitions {
                name: ResourceName("cpu".to_string()),
            },
        ]);

        assert_eq!(body.code, api::error_code::deployment_validation::FAILED);
        assert_eq!(body.errors.len(), 2);
        assert!(body.errors.iter().any(|error| {
            error.starts_with(&format!(
                "{}: ",
                api::error_code::deployment_validation::AGENT_SECRET_TYPE_CONFLICT
            ))
        }));
        assert!(body.errors.iter().any(|error| {
            error.starts_with(&format!(
                "{}: ",
                api::error_code::deployment_validation::CONFLICTING_RESOURCE_DEFINITIONS
            ))
        }));
    }

    #[test]
    fn deployment_validation_reset_override_uses_specific_subcode() {
        let body = bad_request_from_validations(vec![
            DeployValidationError::ResetOverrideRequiresCompatibilityCheckDisabled,
        ]);

        assert_eq!(body.code, api::error_code::deployment_validation::FAILED);
        assert_eq!(body.errors.len(), 1);
        assert!(body.errors[0].starts_with(&format!(
            "{}: ",
            api::error_code::deployment_validation::RESET_OVERRIDE_REQUIRES_COMPATIBILITY_CHECK_DISABLED
        )));
    }

    #[test]
    fn component_reset_override_disabled_maps_to_specific_code() {
        let api_error =
            ApiError::from(ComponentError::ResetOverrideRequiresCompatibilityCheckDisabled);

        match api_error {
            ApiError::Conflict(body) => {
                assert_eq!(
                    body.0.code,
                    api::error_code::RESET_OVERRIDE_REQUIRES_COMPATIBILITY_CHECK_DISABLED
                );
            }
            other => panic!("Expected Conflict, got: {other:?}"),
        }
    }

    #[test]
    fn card_tree_changed_during_revoke_maps_to_concurrent_update() {
        let api_error = ApiError::from(CardError::from(CardRepoError::CardTreeChangedDuringDelete));

        match api_error {
            ApiError::Conflict(body) => {
                assert_eq!(body.0.code, api::error_code::CONCURRENT_UPDATE);
            }
            other => panic!("Expected Conflict, got: {other:?}"),
        }
    }

    fn status_and_code(api_error: ApiError) -> (&'static str, String) {
        match api_error {
            ApiError::BadRequest(body) => ("bad_request", body.0.code),
            ApiError::NotFound(body) => ("not_found", body.0.code),
            other => panic!("Expected bad request or not found, got: {other:?}"),
        }
    }

    #[test]
    fn tool_release_errors_use_specific_codes() {
        use golem_common::model::tool::ToolName;
        use golem_common::model::tool_release::ToolReleaseId;

        let tool_name = ToolName::try_from("echo").unwrap();
        let cases = [
            (
                ToolReleaseError::ToolReleaseNotFound(ToolReleaseId::new()),
                "not_found",
                api::error_code::TOOL_RELEASE_NOT_FOUND,
            ),
            (
                ToolReleaseError::ReferencedToolReleaseNotFound,
                "not_found",
                api::error_code::REFERENCED_TOOL_RELEASE_NOT_FOUND,
            ),
            (
                ToolReleaseError::PublicationToolNotFound(tool_name.clone()),
                "bad_request",
                api::error_code::TOOL_PUBLICATION_TOOL_NOT_FOUND,
            ),
            (
                ToolReleaseError::DuplicatePublication(tool_name.clone()),
                "bad_request",
                api::error_code::DUPLICATE_TOOL_PUBLICATION,
            ),
            (
                ToolReleaseError::PublicationOwnerMismatch(tool_name.clone()),
                "bad_request",
                api::error_code::TOOL_PUBLICATION_OWNER_MISMATCH,
            ),
            (
                ToolReleaseError::PublicationHostSource(tool_name),
                "bad_request",
                api::error_code::TOOL_PUBLICATION_HOST_SOURCE_NOT_SUPPORTED,
            ),
        ];

        for (error, expected_status, expected_code) in cases {
            let (status, code) = status_and_code(error.into());
            assert_eq!(status, expected_status);
            assert_eq!(code, expected_code);
        }
    }

    #[test]
    fn environment_tool_grant_errors_distinguish_grants_from_releases() {
        use golem_common::model::environment_tool_grant::EnvironmentToolGrantId;

        let (status, code) = status_and_code(
            EnvironmentToolGrantError::EnvironmentToolGrantNotFound(EnvironmentToolGrantId::new())
                .into(),
        );
        assert_eq!(status, "not_found");
        assert_eq!(code, api::error_code::ENVIRONMENT_TOOL_GRANT_NOT_FOUND);

        let (status, code) =
            status_and_code(EnvironmentToolGrantError::ReferencedToolReleaseNotFound.into());
        assert_eq!(status, "not_found");
        assert_eq!(code, api::error_code::REFERENCED_TOOL_RELEASE_NOT_FOUND);
    }

    #[test]
    fn deployment_tool_not_found_keeps_tool_not_found_code() {
        use golem_common::model::tool::ToolName;

        let (status, code) = status_and_code(
            DeploymentError::ToolNotFound(ToolName::try_from("echo").unwrap()).into(),
        );
        assert_eq!(status, "not_found");
        assert_eq!(code, api::error_code::TOOL_NOT_FOUND);
    }

    #[test]
    fn deployment_tool_release_immutable_conflict_uses_specific_code() {
        let api_error = ApiError::from(DeploymentWriteError::ToolReleaseImmutableConflict);

        match api_error {
            ApiError::Conflict(body) => {
                assert_eq!(
                    body.0.code,
                    api::error_code::TOOL_RELEASE_IMMUTABLE_CONFLICT
                );
            }
            other => panic!("Expected Conflict, got: {other:?}"),
        }
    }

    #[test]
    fn deployment_tool_release_de_published_conflict_uses_specific_code() {
        let api_error = ApiError::from(DeploymentWriteError::ToolReleaseDePublishedConflict);

        match api_error {
            ApiError::Conflict(body) => {
                assert_eq!(
                    body.0.code,
                    api::error_code::TOOL_RELEASE_LIFECYCLE_CONFLICT
                );
                assert!(body.0.error.contains("de-published"));
            }
            other => panic!("Expected Conflict, got: {other:?}"),
        }
    }
}
