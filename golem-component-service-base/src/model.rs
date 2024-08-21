use golem_common::model::component_metadata::ComponentMetadata;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component<Namespace> {
    pub namespace: Namespace,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
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
            created_at: value.created_at,
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component<Namespace>) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: None,
            created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                value.created_at,
            ))),
        }
    }
}
