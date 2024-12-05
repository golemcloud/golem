use crate::repo::{CloudComponentOwnerRow, CloudPluginScopeRow};
use crate::service::component::CloudComponentService;
use async_trait::async_trait;
use cloud_common::auth::CloudAuthCtx;
use cloud_common::model::CloudPluginOwner;
use golem_common::model::component::ComponentOwner;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::plugin::PluginScope;
use golem_common::model::plugin::{ComponentPluginScope, PluginInstallation};
use golem_common::model::{
    AccountId, ComponentId, ComponentType, Empty, HasAccountId, InitialComponentFile, ProjectId,
};
use golem_common::SafeDisplay;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use poem_openapi::types::{ParseError, ParseFromParameter, ParseResult};
use poem_openapi::{Object, Union};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CloudComponentOwner {
    pub project_id: ProjectId,
    pub account_id: AccountId,
}

impl From<CloudComponentOwner> for CloudPluginOwner {
    fn from(value: CloudComponentOwner) -> Self {
        CloudPluginOwner {
            account_id: value.account_id,
        }
    }
}

impl Display for CloudComponentOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.account_id, self.project_id)
    }
}

impl FromStr for CloudComponentOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid namespace: {s}"));
        }

        Ok(Self {
            project_id: ProjectId::try_from(parts[1])?,
            account_id: AccountId::from(parts[0]),
        })
    }
}

impl HasAccountId for CloudComponentOwner {
    fn account_id(&self) -> AccountId {
        self.account_id.clone()
    }
}

impl ComponentOwner for CloudComponentOwner {
    type Row = CloudComponentOwnerRow;
    type PluginOwner = CloudPluginOwner;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectPluginScope {
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum CloudPluginScope {
    Global(Empty),
    Component(ComponentPluginScope),
    Project(ProjectPluginScope),
}

impl CloudPluginScope {
    pub fn global() -> Self {
        CloudPluginScope::Global(Empty {})
    }

    pub fn component(component_id: ComponentId) -> Self {
        CloudPluginScope::Component(ComponentPluginScope { component_id })
    }

    pub fn project(project_id: ProjectId) -> Self {
        CloudPluginScope::Project(ProjectPluginScope { project_id })
    }

    pub fn valid_in_component(&self, component_id: &ComponentId, project_id: &ProjectId) -> bool {
        match self {
            CloudPluginScope::Global(_) => true,
            CloudPluginScope::Component(scope) => &scope.component_id == component_id,
            CloudPluginScope::Project(scope) => &scope.project_id == project_id,
        }
    }
}

impl Default for CloudPluginScope {
    fn default() -> Self {
        CloudPluginScope::global()
    }
}

impl Display for CloudPluginScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CloudPluginScope::Global(_) => write!(f, "global"),
            CloudPluginScope::Component(scope) => write!(f, "component:{}", scope.component_id),
            CloudPluginScope::Project(scope) => write!(f, "project:{}", scope.project_id),
        }
    }
}

impl ParseFromParameter for CloudPluginScope {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        if value == "global" {
            Ok(Self::global())
        } else if let Some(id_part) = value.strip_prefix("component:") {
            let component_id = ComponentId::try_from(id_part);
            match component_id {
                Ok(component_id) => Ok(Self::component(component_id)),
                Err(err) => Err(ParseError::custom(err)),
            }
        } else if let Some(id_part) = value.strip_prefix("project:") {
            let project_id = ProjectId::try_from(id_part);
            match project_id {
                Ok(project_id) => Ok(Self::project(project_id)),
                Err(err) => Err(ParseError::custom(err)),
            }
        } else {
            Err(ParseError::custom("Unexpected representation of plugin scope - must be 'global', 'component:<component_id>' or 'project:<project_id>'"))
        }
    }
}

impl From<CloudPluginScope> for cloud_api_grpc::proto::golem::cloud::component::CloudPluginScope {
    fn from(scope: CloudPluginScope) -> Self {
        match scope {
            CloudPluginScope::Global(_) => cloud_api_grpc::proto::golem::cloud::component::CloudPluginScope {
                scope: Some(cloud_api_grpc::proto::golem::cloud::component::cloud_plugin_scope::Scope::Global(
                    golem_api_grpc::proto::golem::common::Empty {},
                )),
            },
            CloudPluginScope::Component(scope) => cloud_api_grpc::proto::golem::cloud::component::CloudPluginScope {
                scope: Some(cloud_api_grpc::proto::golem::cloud::component::cloud_plugin_scope::Scope::Component(
                    golem_api_grpc::proto::golem::component::ComponentPluginScope {
                        component_id: Some(scope.component_id.into()),
                    },
                )),
            },
            CloudPluginScope::Project(scope) => cloud_api_grpc::proto::golem::cloud::component::CloudPluginScope {
                scope: Some(cloud_api_grpc::proto::golem::cloud::component::cloud_plugin_scope::Scope::Project(
                    cloud_api_grpc::proto::golem::cloud::component::ProjectPluginScope {
                        project_id: Some(scope.project_id.into()),
                    },
                )),
            },
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::component::CloudPluginScope>
    for CloudPluginScope
{
    type Error = String;

    fn try_from(
        proto: cloud_api_grpc::proto::golem::cloud::component::CloudPluginScope,
    ) -> Result<Self, Self::Error> {
        match proto.scope {
            Some(cloud_api_grpc::proto::golem::cloud::component::cloud_plugin_scope::Scope::Global(
                _,
            )) => Ok(Self::global()),
            Some(cloud_api_grpc::proto::golem::cloud::component::cloud_plugin_scope::Scope::Component(
                scope,
            )) => Ok(Self::component(scope.component_id.ok_or("Missing component_id")?.try_into()?)),
            Some(cloud_api_grpc::proto::golem::cloud::component::cloud_plugin_scope::Scope::Project(
                scope,
            )) => Ok(Self::project(scope.project_id.ok_or("Missing project_id")?.try_into()?)),
            None => Err("Missing scope".to_string()),
        }
    }
}

#[async_trait]
impl PluginScope for CloudPluginScope {
    type Row = CloudPluginScopeRow;

    type RequestContext = (Arc<CloudComponentService>, CloudAuthCtx);

    async fn accessible_scopes(&self, context: Self::RequestContext) -> Result<Vec<Self>, String> {
        match self {
            CloudPluginScope::Global(_) =>
            // In global scope we only have access to plugins in global scope
            {
                Ok(vec![self.clone()])
            }
            CloudPluginScope::Component(component) => {
                // In a component scope we have access to
                // - plugins in that particular scope
                // - plugins of the component's owner project
                // - and all the global ones

                let (component_service, auth_ctx) = context;
                let component = component_service
                    .get_latest_version(&component.component_id, &auth_ctx)
                    .await
                    .map_err(|err| err.to_safe_string())?;

                let project = component.map(|component| component.project_id);

                if let Some(project_id) = project {
                    Ok(vec![
                        Self::global(),
                        Self::project(project_id),
                        self.clone(),
                    ])
                } else {
                    Ok(vec![Self::global(), self.clone()])
                }
            }
            CloudPluginScope::Project(_) =>
            // In a project scope we have access to plugins in that particular scope, and all the global ones
            {
                Ok(vec![Self::global(), self.clone()])
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentQuery {
    pub project_id: Option<ProjectId>,
    pub component_name: ComponentName,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Object)]
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
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
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
            files: component.files,
            installed_plugins: component.installed_plugins,
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
        let files = value
            .files
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        let installed_plugins = value
            .installed_plugins
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>, _>>()?;

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
            files,
            installed_plugins,
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
            files: value.files.into_iter().map(|f| f.into()).collect(),
            installed_plugins: value
                .installed_plugins
                .into_iter()
                .map(|p| p.into())
                .collect(),
        }
    }
}

impl From<golem_component_service_base::model::Component<CloudComponentOwner>> for Component {
    fn from(value: golem_component_service_base::model::Component<CloudComponentOwner>) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            project_id: value.owner.project_id,
            created_at: Some(value.created_at),
            component_type: Some(value.component_type),
            files: value.files,
            installed_plugins: value.installed_plugins,
        }
    }
}
