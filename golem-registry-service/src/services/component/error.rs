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
use golem_common::SafeDisplay;
use golem_common::model::account::AccountId;
use golem_common::model::component::{
    ComponentFilePath, ComponentName, ComponentRevision, InitialComponentFileKey,
    VersionedComponentId,
};
use golem_common::model::component_metadata::ComponentProcessingError;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{ComponentId, PluginInstallationId};
use golem_service_base::repo::RepoError;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component already exists: {0}")]
    AlreadyExists(ComponentId),
    #[error("Unknown component id: {0}")]
    UnknownComponentId(ComponentId),
    #[error("Component {component_name} not found in environment {environment_id}")]
    UnknownEnvironmentComponentName {
        environment_id: EnvironmentId,
        component_name: ComponentName,
    },
    #[error("Unknown versioned component id: {0}")]
    UnknownVersionedComponentId(VersionedComponentId),
    #[error(transparent)]
    ComponentProcessingError(#[from] ComponentProcessingError),
    #[error("Malformed component archive: {message}")]
    MalformedComponentArchive { message: String },
    #[error("Provided component file not found: {path} (key: {key})")]
    InitialComponentFileNotFound { path: String, key: String },
    #[error("Invalid file path: {0}")]
    InvalidFilePath(String),
    #[error(
        "The component name {actual} did not match the component's root package name: {expected}"
    )]
    InvalidComponentName { expected: String, actual: String },
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit {limit_name} exceeded, limit: {limit_value}, current: {current_value}")]
    LimitExceeded {
        limit_name: String,
        limit_value: i64,
        current_value: i64,
    },
    #[error("Plugin does not implement golem:api/oplog-processor")]
    InvalidOplogProcessorPlugin,
    #[error("Plugin not found: {account_id}/{plugin_name}@{plugin_version}")]
    PluginNotFound {
        account_id: AccountId,
        plugin_name: String,
        plugin_version: String,
    },
    #[error("Invalid plugin scope for {plugin_name}@{plugin_version} {details}")]
    InvalidPluginScope {
        plugin_name: String,
        plugin_version: String,
        details: String,
    },
    #[error("Concurrent update of component {component_id} from version {current_revision}")]
    ConcurrentUpdate {
        component_id: ComponentId,
        current_revision: ComponentRevision,
    },
    #[error("Plugin installation not found: {installation_id}")]
    PluginInstallationNotFound {
        installation_id: PluginInstallationId,
    },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl ComponentError {
    pub fn initial_component_file_not_found(
        path: &ComponentFilePath,
        key: &InitialComponentFileKey,
    ) -> Self {
        Self::InitialComponentFileNotFound {
            path: path.to_string(),
            key: key.to_string(),
        }
    }
}

impl SafeDisplay for ComponentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AlreadyExists(_) => self.to_string(),
            Self::UnknownComponentId(_) => self.to_string(),
            Self::UnknownEnvironmentComponentName { .. } => self.to_string(),
            Self::UnknownVersionedComponentId(_) => self.to_string(),
            Self::ComponentProcessingError(inner) => inner.to_safe_string(),
            Self::MalformedComponentArchive { .. } => self.to_string(),
            Self::InitialComponentFileNotFound { .. } => self.to_string(),
            Self::InvalidFilePath(_) => self.to_string(),
            Self::InvalidComponentName { .. } => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::LimitExceeded { .. } => self.to_string(),
            Self::InvalidOplogProcessorPlugin => self.to_string(),
            Self::PluginNotFound { .. } => self.to_string(),
            Self::InvalidPluginScope { .. } => self.to_string(),
            Self::ConcurrentUpdate { .. } => self.to_string(),
            Self::PluginInstallationNotFound { .. } => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl From<RepoError> for ComponentError {
    fn from(value: RepoError) -> Self {
        Self::InternalError(anyhow::Error::new(value).context("from RepoError"))
    }
}

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
            _ => Self::InternalError(anyhow::Error::new(value).context("from AccountUsageError")),
        }
    }
}
