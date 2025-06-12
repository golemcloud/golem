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

use crate::model::ConflictReport;
use crate::service::transformer_plugin_caller::TransformationFailedReason;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_error;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::component_metadata::ComponentProcessingError;
use golem_common::model::ComponentId;
use golem_common::model::{ComponentFilePath, InitialComponentFileKey};
use golem_common::SafeDisplay;
use golem_service_base::clients::auth::AuthServiceError;
use golem_service_base::clients::limit::LimitError;
use golem_service_base::clients::project::ProjectError;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::vec;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component already exists: {0}")]
    AlreadyExists(ComponentId),
    #[error("Unknown component id: {0}")]
    UnknownComponentId(ComponentId),
    #[error("Unknown versioned component id: {0}")]
    UnknownVersionedComponentId(VersionedComponentId),
    #[error(transparent)]
    ComponentProcessingError(#[from] ComponentProcessingError),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error("Internal error: failed to convert {what}: {error}")]
    InternalConversionError { what: String, error: String },
    #[error("Internal component store error: {message}: {error}")]
    ComponentStoreError { message: String, error: String },
    #[error("Component Constraint Error. Make sure the component is backward compatible as the functions are already in use:\n{0}"
    )]
    ComponentConstraintConflictError(ConflictReport),
    #[error("Component Constraint Create Error: {0}")]
    ComponentConstraintCreateError(String),
    #[error("Malformed component archive error: {message}: {error:?}")]
    MalformedComponentArchiveError {
        message: String,
        error: Option<String>,
    },
    #[error("Failed uploading initial component files: {message}: {error}")]
    InitialComponentFileUploadError { message: String, error: String },
    #[error("Provided component file not found: {path} (key: {key})")]
    InitialComponentFileNotFound { path: String, key: String },
    #[error("Component transformation failed: {0}")]
    TransformationFailed(TransformationFailedReason),
    #[error("Plugin composition failed: {0}")]
    PluginApplicationFailed(String),
    #[error("Failed to download file from component")]
    FailedToDownloadFile,
    #[error("Invalid file path: {0}")]
    InvalidFilePath(String),
    #[error(
        "The component name {actual} did not match the component's root package name: {expected}"
    )]
    InvalidComponentName { expected: String, actual: String },
    #[error("Unknown project: {0}")]
    UnknownProject(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
    #[error(transparent)]
    InternalAuthServiceError(AuthServiceError),
    #[error(transparent)]
    InternalLimitError(LimitError),
    #[error(transparent)]
    InternalProjectError(ProjectError),
    #[error("Plugin does not implement golem:api/oplog-processor")]
    InvalidOplogProcessorPlugin,
    #[error("Plugin not found: {plugin_name}@{plugin_version}")]
    PluginNotFound {
        plugin_name: String,
        plugin_version: String,
    },
    #[error("Error accessing blob storage: {0}")]
    BlobStorageError(String),
    #[error("Invalid plugin scope for {plugin_name}@{plugin_version} {details}")]
    InvalidPluginScope {
        plugin_name: String,
        plugin_version: String,
        details: String,
    },
}

impl ComponentError {
    pub fn conversion_error(what: impl AsRef<str>, error: String) -> ComponentError {
        Self::InternalConversionError {
            what: what.as_ref().to_string(),
            error,
        }
    }

    pub fn component_store_error(message: impl AsRef<str>, error: anyhow::Error) -> ComponentError {
        Self::ComponentStoreError {
            message: message.as_ref().to_string(),
            error: format!("{error}"),
        }
    }

    pub fn malformed_component_archive_from_message(message: impl AsRef<str>) -> Self {
        Self::MalformedComponentArchiveError {
            message: message.as_ref().to_string(),
            error: None,
        }
    }

    pub fn malformed_component_archive_from_error(
        message: impl AsRef<str>,
        error: anyhow::Error,
    ) -> Self {
        Self::MalformedComponentArchiveError {
            message: message.as_ref().to_string(),
            error: Some(format!("{error}")),
        }
    }

    pub fn initial_component_file_upload_error(
        message: impl AsRef<str>,
        error: impl AsRef<str>,
    ) -> Self {
        Self::InitialComponentFileUploadError {
            message: message.as_ref().to_string(),
            error: error.as_ref().to_string(),
        }
    }

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
            ComponentError::AlreadyExists(_) => self.to_string(),
            ComponentError::UnknownComponentId(_) => self.to_string(),
            ComponentError::UnknownVersionedComponentId(_) => self.to_string(),
            ComponentError::ComponentProcessingError(inner) => inner.to_safe_string(),
            ComponentError::InternalRepoError(inner) => inner.to_safe_string(),
            ComponentError::InternalConversionError { .. } => self.to_string(),
            ComponentError::ComponentStoreError { .. } => self.to_string(),
            ComponentError::ComponentConstraintConflictError(_) => self.to_string(),
            ComponentError::ComponentConstraintCreateError(_) => self.to_string(),
            ComponentError::MalformedComponentArchiveError { .. } => self.to_string(),
            ComponentError::InitialComponentFileUploadError { .. } => self.to_string(),
            ComponentError::InitialComponentFileNotFound { .. } => self.to_string(),
            ComponentError::TransformationFailed(_) => self.to_string(),
            ComponentError::PluginApplicationFailed(_) => self.to_string(),
            ComponentError::FailedToDownloadFile => self.to_string(),
            ComponentError::InvalidFilePath(_) => self.to_string(),
            ComponentError::InvalidComponentName { .. } => self.to_string(),
            ComponentError::UnknownProject(_) => self.to_string(),
            ComponentError::Unauthorized(_) => self.to_string(),
            ComponentError::LimitExceeded(_) => self.to_string(),
            ComponentError::InternalAuthServiceError(inner) => inner.to_safe_string(),
            ComponentError::InternalLimitError(inner) => inner.to_safe_string(),
            ComponentError::InternalProjectError(inner) => inner.to_safe_string(),
            ComponentError::InvalidOplogProcessorPlugin => self.to_string(),
            ComponentError::PluginNotFound { .. } => self.to_string(),
            ComponentError::BlobStorageError(_) => self.to_string(),
            ComponentError::InvalidPluginScope { .. } => self.to_string(),
        }
    }
}

impl From<RepoError> for ComponentError {
    fn from(error: RepoError) -> Self {
        ComponentError::InternalRepoError(error)
    }
}

impl From<ComponentError> for golem_api_grpc::proto::golem::component::v1::ComponentError {
    fn from(value: ComponentError) -> Self {
        let error = match value {
            ComponentError::AlreadyExists(_) => component_error::Error::AlreadyExists(ErrorBody {
                error: value.to_safe_string(),
            }),

            ComponentError::Unauthorized(_) => component_error::Error::Unauthorized(ErrorBody {
                error: value.to_safe_string(),
            }),

            ComponentError::LimitExceeded(_) => component_error::Error::LimitExceeded(ErrorBody {
                error: value.to_safe_string(),
            }),

            ComponentError::UnknownComponentId(_)
            | ComponentError::UnknownVersionedComponentId(_)
            | ComponentError::PluginNotFound { .. }
            | ComponentError::InitialComponentFileNotFound { .. }
            | ComponentError::UnknownProject(_) => component_error::Error::NotFound(ErrorBody {
                error: value.to_safe_string(),
            }),

            ComponentError::ComponentConstraintConflictError(_)
            | ComponentError::ComponentConstraintCreateError(_)
            | ComponentError::InvalidOplogProcessorPlugin
            | ComponentError::MalformedComponentArchiveError { .. }
            | ComponentError::InvalidComponentName { .. }
            | ComponentError::ComponentProcessingError(_)
            | ComponentError::InvalidPluginScope { .. } => {
                component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                })
            }

            ComponentError::InternalAuthServiceError(_)
            | ComponentError::InternalLimitError(_)
            | ComponentError::InternalProjectError(_)
            | ComponentError::InvalidFilePath(_)
            | ComponentError::PluginApplicationFailed(_)
            | ComponentError::FailedToDownloadFile
            | ComponentError::TransformationFailed(_)
            | ComponentError::InitialComponentFileUploadError { .. }
            | ComponentError::ComponentStoreError { .. }
            | ComponentError::InternalConversionError { .. }
            | ComponentError::InternalRepoError(_)
            | ComponentError::BlobStorageError(_) => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        Self { error: Some(error) }
    }
}

impl From<AuthServiceError> for ComponentError {
    fn from(error: AuthServiceError) -> Self {
        match error {
            AuthServiceError::Unauthorized(error) => ComponentError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => ComponentError::Unauthorized(error),
            _ => ComponentError::InternalAuthServiceError(error),
        }
    }
}

impl From<LimitError> for ComponentError {
    fn from(error: LimitError) -> Self {
        match error {
            LimitError::Unauthorized(string) => ComponentError::Unauthorized(string),
            LimitError::LimitExceeded(string) => ComponentError::LimitExceeded(string),
            _ => ComponentError::InternalLimitError(error),
        }
    }
}

impl From<ProjectError> for ComponentError {
    fn from(error: ProjectError) -> Self {
        use golem_api_grpc::proto::golem::project::v1::project_error;

        match error {
            ProjectError::Server(golem_api_grpc::proto::golem::project::v1::ProjectError {
                error: Some(project_error::Error::Unauthorized(e)),
            }) => ComponentError::Unauthorized(e.error),
            ProjectError::Server(golem_api_grpc::proto::golem::project::v1::ProjectError {
                error: Some(project_error::Error::LimitExceeded(e)),
            }) => ComponentError::LimitExceeded(e.error),
            ProjectError::Server(golem_api_grpc::proto::golem::project::v1::ProjectError {
                error: Some(project_error::Error::NotFound(e)),
            }) => ComponentError::UnknownProject(e.error),
            _ => ComponentError::InternalProjectError(error),
        }
    }
}
