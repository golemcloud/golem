use cloud_api_grpc::proto::golem::cloud::project::{Project, ProjectData};
use golem_common::model::{AccountId, ProjectId};
use golem_service_base::model::{
    ComponentMetadata, ComponentName, ProtectedComponentId, UserComponentId, VersionedComponentId,
};
use poem_openapi::Object;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentQuery {
    pub project_id: Option<ProjectId>,
    pub component_name: ComponentName,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub user_component_id: UserComponentId,
    pub protected_component_id: ProtectedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub project_id: ProjectId,
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
            user_component_id: value
                .user_component_id
                .ok_or("Missing user_component_id")?
                .try_into()?,
            protected_component_id: value
                .protected_component_id
                .ok_or("Missing protected_component_id")?
                .try_into()?,
            component_name: ComponentName(value.component_name),
            component_size: value.component_size,
            metadata: value.metadata.ok_or("Missing metadata")?.try_into()?,
            project_id: value.project_id.ok_or("Missing project_id")?.try_into()?,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            user_component_id: Some(value.user_component_id.into()),
            protected_component_id: Some(value.protected_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: Some(value.project_id.into()),
        }
    }
}

impl Component {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectView {
    pub id: ProjectId,
    pub owner_account_id: AccountId,
    pub name: String,
    pub description: String,
}

impl TryFrom<Project> for ProjectView {
    type Error = String;

    fn try_from(value: Project) -> Result<Self, Self::Error> {
        let ProjectData {
            name,
            description,
            owner_account_id,
            ..
        } = value.data.ok_or("Missing data")?;
        Ok(Self {
            id: value.id.ok_or("Missing id")?.try_into()?,
            owner_account_id: owner_account_id.ok_or("Missing owner_account_id")?.into(),
            name,
            description,
        })
    }
}
