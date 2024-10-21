use std::path::PathBuf;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::ComponentType;
use golem_service_base::model::{ComponentName, InitialFilePermission, VersionedComponentId};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use crate::repo::component::InitialFileRecord;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component<Namespace> {
    pub namespace: Namespace,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
}

impl<Namespace> Component<Namespace> {
    pub fn next_version(self) -> Self {
        let new_version = VersionedComponentId {
            component_id: self.versioned_component_id.component_id,
            version: self.versioned_component_id.version + 1,
        };
        Self {
            versioned_component_id: new_version.clone(),
            ..self
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_service_base::model::Component {
    fn from(value: Component<Namespace>) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: Some(value.created_at),
            component_type: Some(value.component_type),
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component<Namespace>) -> Self {
        let component_type: golem_api_grpc::proto::golem::component::ComponentType =
            value.component_type.into();
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: None,
            created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                value.created_at,
            ))),
            component_type: Some(component_type.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialFile {
    pub versioned_component_id: VersionedComponentId,
    pub file_path: PathBuf,
    pub file_permission: InitialFilePermission,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub blob_storage_id: String,
}

impl From<InitialFile> for InitialFileRecord {
    fn from(value: InitialFile) -> Self {
        Self {
            component_id: value.versioned_component_id.component_id.0,
            version: value.versioned_component_id.version as i64,
            file_path: value.file_path.to_string_lossy().to_string(),
            file_permission: value.file_permission.into(),
            created_at: value.created_at,
            blob_storage_id: value.blob_storage_id,
        }
    }
}
