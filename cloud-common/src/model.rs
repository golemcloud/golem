use crate::auth::{CloudAuthCtx, CloudNamespace};
use crate::repo::component::CloudComponentOwnerRow;
use crate::repo::plugin::CloudPluginScopeRow;
use crate::repo::CloudPluginOwnerRow;
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::project::{Project, ProjectData};
use golem_common::model::component::ComponentOwner;
use golem_common::model::plugin::{ComponentPluginScope, PluginOwner, PluginScope};
use golem_common::model::{AccountId, ComponentId, Empty, ProjectId};
use golem_common::newtype_uuid;
use poem::web::Field;
use poem_openapi::types::{ParseError, ParseFromMultipartField, ParseFromParameter, ParseResult};
use poem_openapi::{Enum, Object};
use poem_openapi_derive::Union;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, FromRepr};
use uuid::Uuid;

newtype_uuid!(PlanId, cloud_api_grpc::proto::golem::cloud::plan::PlanId);
newtype_uuid!(
    ProjectGrantId,
    cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantId
);
newtype_uuid!(
    ProjectPolicyId,
    cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectPolicyId
);
newtype_uuid!(TokenId, cloud_api_grpc::proto::golem::cloud::token::TokenId);

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct TokenSecret {
    pub value: Uuid,
}

impl TokenSecret {
    pub fn new(value: Uuid) -> Self {
        Self { value }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::token::TokenSecret> for TokenSecret {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::token::TokenSecret,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            value: value.value.ok_or("Missing field: value")?.into(),
        })
    }
}

impl From<TokenSecret> for cloud_api_grpc::proto::golem::cloud::token::TokenSecret {
    fn from(value: TokenSecret) -> Self {
        Self {
            value: Some(value.value.into()),
        }
    }
}

impl std::str::FromStr for TokenSecret {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(s).map_err(|err| format!("Invalid token: {err}"))?;
        Ok(Self { value: uuid })
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
    Enum,
    EnumIter,
    FromRepr,
)]
#[repr(i32)]
pub enum ProjectAction {
    ViewComponent = 0,
    CreateComponent = 1,
    UpdateComponent = 2,
    DeleteComponent = 3,
    ViewWorker = 4,
    CreateWorker = 5,
    UpdateWorker = 6,
    DeleteWorker = 7,
    ViewProjectGrants = 8,
    CreateProjectGrants = 9,
    DeleteProjectGrants = 10,
    ViewApiDefinition = 11,
    CreateApiDefinition = 12,
    UpdateApiDefinition = 13,
    DeleteApiDefinition = 14,
    DeleteProject = 15,
    ViewProject = 16,
    ViewPluginInstallations = 17,
    CreatePluginInstallation = 18,
    UpdatePluginInstallation = 19,
    DeletePluginInstallation = 20,
    UpsertApiDeployment = 21,
    ViewApiDeployment = 22,
    DeleteApiDeployment = 23,
    UpsertApiDomain = 24,
    ViewApiDomain = 25,
    DeleteApiDomain = 26,
    BatchUpdatePluginInstallations = 27,
}

impl From<ProjectAction> for i32 {
    fn from(value: ProjectAction) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for ProjectAction {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        ProjectAction::from_repr(value).ok_or_else(|| format!("Invalid project action: {}", value))
    }
}

impl Display for ProjectAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            ProjectAction::ViewComponent => write!(f, "ViewComponent"),
            ProjectAction::CreateComponent => write!(f, "CreateComponent"),
            ProjectAction::UpdateComponent => write!(f, "UpdateComponent"),
            ProjectAction::DeleteComponent => write!(f, "DeleteComponent"),
            ProjectAction::ViewWorker => write!(f, "ViewWorker"),
            ProjectAction::CreateWorker => write!(f, "CreateWorker"),
            ProjectAction::UpdateWorker => write!(f, "UpdateWorker"),
            ProjectAction::DeleteWorker => write!(f, "DeleteWorker"),
            ProjectAction::ViewProjectGrants => write!(f, "ViewProjectGrants"),
            ProjectAction::CreateProjectGrants => write!(f, "CreateProjectGrants"),
            ProjectAction::DeleteProjectGrants => write!(f, "DeleteProjectGrants"),
            ProjectAction::ViewApiDefinition => write!(f, "ViewApiDefinition"),
            ProjectAction::CreateApiDefinition => write!(f, "CreateApiDefinition"),
            ProjectAction::UpdateApiDefinition => write!(f, "UpdateApiDefinition"),
            ProjectAction::DeleteApiDefinition => write!(f, "DeleteApiDefinition"),
            ProjectAction::DeleteProject => write!(f, "DeleteProject"),
            ProjectAction::ViewProject => write!(f, "ViewProject"),
            ProjectAction::ViewPluginInstallations => write!(f, "ViewPluginInstallations"),
            ProjectAction::CreatePluginInstallation => write!(f, "CreatePluginInstallation"),
            ProjectAction::UpdatePluginInstallation => write!(f, "UpdatePluginInstallation"),
            ProjectAction::DeletePluginInstallation => write!(f, "DeletePluginInstallation"),
            ProjectAction::UpsertApiDeployment => write!(f, "UpsertApiDeployment"),
            ProjectAction::ViewApiDeployment => write!(f, "ViewApiDeployment"),
            ProjectAction::DeleteApiDeployment => write!(f, "DeleteApiDeployment"),
            ProjectAction::UpsertApiDomain => write!(f, "UpsertApiDomain"),
            ProjectAction::ViewApiDomain => write!(f, "ViewApiDomain"),
            ProjectAction::DeleteApiDomain => write!(f, "DeleteApiDomain"),
            ProjectAction::BatchUpdatePluginInstallations => {
                write!(f, "BatchUpdatePluginInstallations")
            }
        }
    }
}

impl FromStr for ProjectAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ViewComponent" => Ok(ProjectAction::ViewComponent),
            "CreateComponent" => Ok(ProjectAction::CreateComponent),
            "UpdateComponent" => Ok(ProjectAction::UpdateComponent),
            "DeleteComponent" => Ok(ProjectAction::DeleteComponent),
            "ViewWorker" => Ok(ProjectAction::ViewWorker),
            "CreateWorker" => Ok(ProjectAction::CreateWorker),
            "UpdateWorker" => Ok(ProjectAction::UpdateWorker),
            "DeleteWorker" => Ok(ProjectAction::DeleteWorker),
            "ViewProjectGrants" => Ok(ProjectAction::ViewProjectGrants),
            "CreateProjectGrants" => Ok(ProjectAction::CreateProjectGrants),
            "DeleteProjectGrants" => Ok(ProjectAction::DeleteProjectGrants),
            "ViewApiDefinition" => Ok(ProjectAction::ViewApiDefinition),
            "CreateApiDefinition" => Ok(ProjectAction::CreateApiDefinition),
            "UpdateApiDefinition" => Ok(ProjectAction::UpdateApiDefinition),
            "DeleteApiDefinition" => Ok(ProjectAction::DeleteApiDefinition),
            "DeleteProject" => Ok(ProjectAction::DeleteProject),
            "ViewProject" => Ok(ProjectAction::ViewProject),
            "ViewPluginInstallations" => Ok(ProjectAction::ViewPluginInstallations),
            "CreatePluginInstallation" => Ok(ProjectAction::CreatePluginInstallation),
            "UpdatePluginInstallation" => Ok(ProjectAction::UpdatePluginInstallation),
            "DeletePluginInstallation" => Ok(ProjectAction::DeletePluginInstallation),
            "UpsertApiDeployment" => Ok(ProjectAction::UpsertApiDeployment),
            "ViewApiDeployment" => Ok(ProjectAction::ViewApiDeployment),
            "DeleteApiDeployment" => Ok(ProjectAction::DeleteApiDeployment),
            "UpsertApiDomain" => Ok(ProjectAction::UpsertApiDomain),
            "ViewApiDomain" => Ok(ProjectAction::ViewApiDomain),
            "DeleteApiDomain" => Ok(ProjectAction::DeleteApiDomain),
            "BatchUpdatePluginInstallations" => Ok(ProjectAction::BatchUpdatePluginInstallations),
            _ => Err(format!("Unknown project action: {}", s)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct ProjectActions {
    pub actions: HashSet<ProjectAction>,
}

impl ProjectActions {
    pub fn empty() -> ProjectActions {
        ProjectActions {
            actions: HashSet::new(),
        }
    }

    pub fn all() -> ProjectActions {
        let actions: HashSet<ProjectAction> =
            ProjectAction::iter().collect::<HashSet<ProjectAction>>();
        ProjectActions { actions }
    }
}

impl From<ProjectActions> for cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectActions {
    fn from(value: ProjectActions) -> Self {
        Self {
            actions: value
                .actions
                .into_iter()
                .map(|action| action.into())
                .collect(),
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectActions>
    for ProjectActions
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectActions,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            actions: value
                .actions
                .into_iter()
                .map(|action| action.try_into())
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct ProjectAuthorisedActions {
    pub project_id: ProjectId,
    pub owner_account_id: AccountId,
    pub actions: ProjectActions,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
    Enum,
    EnumIter,
    FromRepr,
)]
#[repr(i32)]
pub enum Role {
    Admin = 0,
    MarketingAdmin = 1,
}

impl From<Role> for i32 {
    fn from(value: Role) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for Role {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Role::from_repr(value).ok_or_else(|| format!("Invalid role: {}", value))
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            _ => Err(format!("Unknown role id: {}", s)),
        }
    }
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Role::Admin => write!(f, "Admin"),
            Role::MarketingAdmin => write!(f, "MarketingAdmin"),
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

impl From<ProjectView> for CloudNamespace {
    fn from(value: ProjectView) -> Self {
        CloudNamespace::new(value.id, value.owner_account_id)
    }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CloudPluginOwner {
    pub account_id: AccountId,
}

impl Display for CloudPluginOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.account_id)
    }
}

impl FromStr for CloudPluginOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            account_id: AccountId::from(s),
        })
    }
}

impl PluginOwner for CloudPluginOwner {
    type Row = CloudPluginOwnerRow;
    fn account_id(&self) -> AccountId {
        self.account_id.clone()
    }
}

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

impl ComponentOwner for CloudComponentOwner {
    type Row = CloudComponentOwnerRow;
    type PluginOwner = CloudPluginOwner;

    fn account_id(&self) -> AccountId {
        self.account_id.clone()
    }
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

impl ParseFromMultipartField for CloudPluginScope {
    async fn parse_from_multipart(field: Option<Field>) -> ParseResult<Self> {
        use poem_openapi::types::ParseFromParameter;
        match field {
            Some(field) => {
                let s = field.text().await?;
                Self::parse_from_parameter(&s)
            }
            None => Err(ParseError::expected_input()),
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
pub trait ComponentOwnershipQuery: Send + Sync {
    async fn get_project(
        &self,
        component_id: &ComponentId,
        auth_ctx: &CloudAuthCtx,
    ) -> Result<Option<ProjectId>, String>;
}

#[async_trait]
impl PluginScope for CloudPluginScope {
    type Row = CloudPluginScopeRow;

    type RequestContext = (Arc<dyn ComponentOwnershipQuery>, CloudAuthCtx);

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
                let project = component_service
                    .get_project(&component.component_id, &auth_ctx)
                    .await?;

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

#[cfg(test)]
mod test {
    use super::*;
    use test_r::test;

    #[test]
    fn role_to_from() {
        for role in Role::iter() {
            let role_as_i32: i32 = role.clone().into();
            let deserialized_role = Role::try_from(role_as_i32).unwrap();
            assert_eq!(role, deserialized_role);

            let role_as_str = role.to_string();
            let deserialized_role = Role::from_str(&role_as_str).unwrap();
            assert_eq!(role, deserialized_role);
            assert_eq!(role, deserialized_role);
        }
    }

    #[test]
    fn project_action_to_from() {
        for action in ProjectAction::iter() {
            let action_as_i32: i32 = action.clone().into();
            let deserialized_action = ProjectAction::try_from(action_as_i32).unwrap();
            assert_eq!(action, deserialized_action);

            let action_as_str = action.to_string();
            let deserialized_action = ProjectAction::from_str(&action_as_str).unwrap();
            assert_eq!(action, deserialized_action);
        }
    }
}
