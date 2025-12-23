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

use crate::repo::model::component::ComponentRepoError;
use crate::services::account_usage::error::{AccountUsageError, LimitExceededError};
use crate::services::application::ApplicationError;
use crate::services::component_transformer_plugin_caller::TransformationFailedReason;
use crate::services::deployment::DeploymentError;
use crate::services::environment::EnvironmentError;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantError;
use crate::services::plugin_registration::PluginRegistrationError;
use golem_common::model::component::PluginPriority;
use golem_common::model::component::{ComponentFileContentHash, ComponentFilePath};
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
    #[error("Provided component file not found: {path} (key: {key})")]
    InitialComponentFileNotFound {
        path: ComponentFilePath,
        key: ComponentFileContentHash,
    },
    #[error("Invalid file path: {0}")]
    InvalidFilePath(String),
    #[error(
        "The component name {actual} did not match the component's root package name: {expected}"
    )]
    InvalidComponentName { expected: String, actual: String },
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
    #[error("Failed to componse component with plugin with priority {plugin_priority}")]
    PluginCompositionFailed {
        plugin_priority: PluginPriority,
        cause: anyhow::Error,
    },
    #[error("Component transformer plugin with priority {plugin_priority} failed with: {reason}")]
    ComponentTransformerPluginFailed {
        plugin_priority: PluginPriority,
        reason: TransformationFailedReason,
    },
    #[error("agent type for name {0} not found in environment")]
    AgentTypeForNameNotFound(String),
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
            Self::InitialComponentFileNotFound { .. } => self.to_string(),
            Self::InvalidFilePath(_) => self.to_string(),
            Self::InvalidComponentName { .. } => self.to_string(),
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
            Self::PluginCompositionFailed { .. } => self.to_string(),
            Self::ComponentTransformerPluginFailed { .. } => self.to_string(),
            Self::ComponentNotFound(_) => self.to_string(),
            Self::ComponentByNameNotFound(_) => self.to_string(),
            Self::AgentTypeForNameNotFound(_) => self.to_string(),
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
