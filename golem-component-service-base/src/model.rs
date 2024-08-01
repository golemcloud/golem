use golem_common::component_metadata::ComponentMetadata;
use golem_service_base::model::{
    ComponentName, ProtectedComponentId, UserComponentId, VersionedComponentId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component<Namespace> {
    pub namespace: Namespace,
    pub versioned_component_id: VersionedComponentId,
    pub user_component_id: UserComponentId,
    pub protected_component_id: ProtectedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
}

impl<Namespace> Component<Namespace> {
    pub fn next_version(self) -> Self {
        let new_version = VersionedComponentId {
            component_id: self.versioned_component_id.component_id,
            version: self.versioned_component_id.version + 1,
        };
        Self {
            versioned_component_id: new_version.clone(),
            user_component_id: UserComponentId {
                versioned_component_id: new_version.clone(),
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: new_version,
            },
            ..self
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_service_base::model::Component {
    fn from(value: Component<Namespace>) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            user_component_id: value.user_component_id,
            protected_component_id: value.protected_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component<Namespace>) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            user_component_id: Some(value.user_component_id.into()),
            protected_component_id: Some(value.protected_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: None,
        }
    }
}
