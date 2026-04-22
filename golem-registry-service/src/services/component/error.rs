// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use crate::repo::model::component::ComponentRepoError;
use crate::services::account_usage::error::{AccountUsageError, LimitExceededError};
use crate::services::application::ApplicationError;
use crate::services::deployment::DeploymentError;
use crate::services::environment::EnvironmentError;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantError;
use crate::services::plugin_registration::PluginRegistrationError;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ArchiveFilePath, PluginPriority};
use golem_common::model::component::{ComponentId, ComponentName};
use golem_common::model::component_metadata::ComponentProcessingError;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::{IntoAnyhow, SafeDisplay, error_forwarding};
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::repo::RepoError;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component with this name already exists in the environment: {0}")]
    ComponentWithNameAlreadyExists(ComponentName),
    #[error("This version already exists for the component: {0}")]
    ComponentVersionAlreadyExists(String),
    #[error(transparent)]
    ComponentProcessingError(#[from] ComponentProcessingError),
    #[error("Malformed component archive: {message}")]
    MalformedComponentArchive { message: String },
    #[error(
        "Agent file with archive path '{archive_path}' not found in the uploaded archive (agent type: {agent_type})"
    )]
    AgentFileNotFoundInArchive {
        agent_type: AgentTypeName,
        archive_path: ArchiveFilePath,
    },
    #[error("Invalid file path: {0}")]
    InvalidFilePath(String),
    #[error("Plugin does not implement golem:api/oplog-processor")]
    InvalidOplogProcessorPlugin,
    #[error("Invalid plugin scope for {plugin_name}@{plugin_version} {details}")]
    InvalidPluginScope {
        plugin_name: String,
        plugin_version: String,
        details: String,
    },
    #[error("Concurrent update of component")]
    ConcurrentUpdate,
    #[error("Environment not found: {0}")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Deployment revision {0} not found")]
    DeploymentRevisionNotFound(DeploymentRevision),
    #[error("Component for id {0} not found")]
    ComponentNotFound(ComponentId),
    #[error("Component for name {0} not found in environment")]
    ComponentByNameNotFound(ComponentName),
    #[error("Plugin not found in the environment for grant id: {0}")]
    EnvironmentPluginNotFound(EnvironmentPluginGrantId),
    #[error("Referenced plugin installation with grant id {0} not found")]
    PluginInstallationNotFound(EnvironmentPluginGrantId),
    #[error("Multiple plugins with same priority {0}")]
    ConflictingPluginPriority(PluginPriority),
    #[error("Multiple plugins with same environment plugin grant id {0}")]
    ConflictingEnvironmentPluginGrantId(EnvironmentPluginGrantId),
    #[error("agent type for name {0} not found in environment")]
    AgentTypeForNameNotFound(String),
    #[error(
        "Agent type '{0}' is referenced in provision config but not declared in the component's agent types"
    )]
    UndeclaredAgentTypeInProvisionConfig(AgentTypeName),
    #[error("Config for agent {agent} at key {rendered_key} is not declared", rendered_key = key.join("."))]
    AgentConfigNotDeclared {
        agent: AgentTypeName,
        key: Vec<String>,
    },
    #[error(
        "Config for agent {agent} at key {rendered_key} has the wrong type: [{rendered_errors}]",
        rendered_key = key.join("."),
        rendered_errors = errors.join(", ")
    )]
    AgentConfigTypeMismatch {
        agent: AgentTypeName,
        key: Vec<String>,
        errors: Vec<String>,
    },
    #[error("Config for agent {agent} at path {rendered_key} is secret and cannot be provided here", rendered_key = path.join("."))]
    AgentConfigProvidedSecretWhereOnlyLocalAllowed {
        agent: AgentTypeName,
        path: Vec<String>,
    },
    #[error("Multiple config values for agent {agent} at config path {rendered_key} provided", rendered_key = path.join("."))]
    AgentConfigDuplicateValue {
        agent: AgentTypeName,
        path: Vec<String>,
    },
    #[error("Old config value for agent {agent} at config key {rendered_key} is no longer valid due to an updated agent.", rendered_key = key.join("."))]
    AgentConfigOldConfigNotValid {
        agent: AgentTypeName,
        key: Vec<String>,
    },
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    LimitExceeded(#[from] LimitExceededError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ComponentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ComponentWithNameAlreadyExists(_) => self.to_string(),
            Self::ComponentVersionAlreadyExists(_) => self.to_string(),
            Self::ComponentProcessingError(inner) => inner.to_safe_string(),
            Self::MalformedComponentArchive { .. } => self.to_string(),
            Self::AgentFileNotFoundInArchive { .. } => self.to_string(),
            Self::InvalidFilePath(_) => self.to_string(),
            Self::LimitExceeded(inner) => inner.to_safe_string(),
            Self::InvalidOplogProcessorPlugin => self.to_string(),
            Self::EnvironmentPluginNotFound(_) => self.to_string(),
            Self::InvalidPluginScope { .. } => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::PluginInstallationNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::DeploymentRevisionNotFound(_) => self.to_string(),
            Self::ConflictingEnvironmentPluginGrantId(_) => self.to_string(),
            Self::ConflictingPluginPriority(_) => self.to_string(),
            Self::ComponentNotFound(_) => self.to_string(),
            Self::ComponentByNameNotFound(_) => self.to_string(),
            Self::AgentTypeForNameNotFound(_) => self.to_string(),
            Self::UndeclaredAgentTypeInProvisionConfig(_) => self.to_string(),
            Self::AgentConfigNotDeclared { .. } => self.to_string(),
            Self::AgentConfigTypeMismatch { .. } => self.to_string(),
            Self::AgentConfigProvidedSecretWhereOnlyLocalAllowed { .. } => self.to_string(),
            Self::AgentConfigDuplicateValue { .. } => self.to_string(),
            Self::AgentConfigOldConfigNotValid { .. } => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    ComponentError,
    RepoError,
    ApplicationError,
    ComponentRepoError,
    EnvironmentError,
    EnvironmentPluginGrantError,
    PluginRegistrationError,
    DeploymentError,
);

impl From<AccountUsageError> for ComponentError {
    fn from(value: AccountUsageError) -> Self {
        match value {
            AccountUsageError::LimitExceeded(inner) => Self::LimitExceeded(inner),
            other => Self::InternalError(other.into_anyhow()),
        }
    }
}
