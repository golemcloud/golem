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

use crate::model::auth::AuthorizationError;
use crate::repo::model::component::ComponentRepoError;
use crate::services::account_usage::error::AccountUsageError;
use crate::services::application::ApplicationError;
use crate::services::component_transformer_plugin_caller::TransformationFailedReason;
use crate::services::environment::EnvironmentError;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantError;
use crate::services::plugin_registration::PluginRegistrationError;
use golem_common::model::component::ComponentId;
use golem_common::model::component::{ComponentFilePath, InitialComponentFileKey};
use golem_common::model::component_metadata::ComponentProcessingError;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::{IntoAnyhow, SafeDisplay, error_forwarding};
use golem_service_base::repo::RepoError;
use golem_common::model::component::PluginPriority;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component already exists: {0}")]
    AlreadyExists(ComponentId),
    #[error(transparent)]
    ComponentProcessingError(#[from] ComponentProcessingError),
    #[error("Malformed component archive: {message}")]
    MalformedComponentArchive { message: String },
    #[error("Provided component file not found: {path} (key: {key})")]
    InitialComponentFileNotFound {
        path: ComponentFilePath,
        key: InitialComponentFileKey,
    },
    #[error("Invalid file path: {0}")]
    InvalidFilePath(String),
    #[error(
        "The component name {actual} did not match the component's root package name: {expected}"
    )]
    InvalidComponentName { expected: String, actual: String },
    #[error("Limit {limit_name} exceeded, limit: {limit_value}, current: {current_value}")]
    LimitExceeded {
        limit_name: String,
        limit_value: i64,
        current_value: i64,
    },
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
    #[error("Current revision for update is incorrect")]
    InvalidCurrentRevision,
    #[error("Environment not found: {0}")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Requested component not found")]
    NotFound,
    #[error("Plugin not found in the environment for id: {0}")]
    EnvironmentPluginNotFound(EnvironmentPluginGrantId),
    #[error("Referenced plugin installation with priority {0} not found")]
    PluginInstallationNotFound(PluginPriority),
    #[error("Multiple plugins with same priority {0}")]
    ConflictingPluginPriority(PluginPriority),
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
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ComponentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AlreadyExists(_) => self.to_string(),
            Self::ComponentProcessingError(inner) => inner.to_safe_string(),
            Self::MalformedComponentArchive { .. } => self.to_string(),
            Self::InitialComponentFileNotFound { .. } => self.to_string(),
            Self::InvalidFilePath(_) => self.to_string(),
            Self::InvalidComponentName { .. } => self.to_string(),
            Self::LimitExceeded { .. } => self.to_string(),
            Self::InvalidOplogProcessorPlugin => self.to_string(),
            Self::EnvironmentPluginNotFound(_) => self.to_string(),
            Self::InvalidPluginScope { .. } => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::InvalidCurrentRevision => self.to_string(),
            Self::PluginInstallationNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::ConflictingPluginPriority(_) => self.to_string(),
            Self::PluginCompositionFailed { .. } => self.to_string(),
            Self::ComponentTransformerPluginFailed { .. } => self.to_string(),
            Self::NotFound => self.to_string(),
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
);

impl From<AccountUsageError> for ComponentError {
    fn from(value: AccountUsageError) -> Self {
        match value {
            AccountUsageError::LimitExceeded {
                limit_name,
                limit_value,
                current_value,
            } => Self::LimitExceeded {
                limit_name,
                limit_value,
                current_value,
            },
            other => Self::InternalError(other.into_anyhow().context("AccountUsageError")),
        }
    }
}
