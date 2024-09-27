use cloud_common::auth::CloudNamespace;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::{ComponentType, ProjectId};
use golem_service_base::model::{ComponentName, VersionedComponentId};
use poem_openapi::Object;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentQuery {
    pub project_id: Option<ProjectId>,
    pub component_name: ComponentName,
    pub component_type: Option<ComponentType>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub project_id: ProjectId,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub component_type: Option<ComponentType>,
}

impl Component {
    pub fn new(component: golem_service_base::model::Component, project_id: ProjectId) -> Self {
        Self {
            versioned_component_id: component.versioned_component_id,
            component_name: component.component_name,
            component_size: component.component_size,
            metadata: component.metadata,
            project_id,
            created_at: component.created_at,
            component_type: component.component_type,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        let component_type = if value.component_type.is_some() {
            Some(value.component_type().into())
        } else {
            None
        };
        let created_at = match value.created_at {
            Some(t) => {
                let t = SystemTime::try_from(t).map_err(|_| "Failed to convert timestamp")?;
                Some(t.into())
            }
            None => None,
        };
        Ok(Self {
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
            component_name: ComponentName(value.component_name),
            component_size: value.component_size,
            metadata: value.metadata.ok_or("Missing metadata")?.try_into()?,
            project_id: value.project_id.ok_or("Missing project_id")?.try_into()?,
            created_at,
            component_type,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: Some(value.project_id.into()),
            created_at: value
                .created_at
                .map(|t| prost_types::Timestamp::from(SystemTime::from(t))),
            component_type: value.component_type.map(|c| {
                let c: golem_api_grpc::proto::golem::component::ComponentType = c.into();
                c.into()
            }),
        }
    }
}

impl From<golem_component_service_base::model::Component<CloudNamespace>> for Component {
    fn from(value: golem_component_service_base::model::Component<CloudNamespace>) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            project_id: value.namespace.project_id,
            created_at: Some(value.created_at),
            component_type: Some(value.component_type),
        }
    }
}
