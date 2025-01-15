use std::collections::HashSet;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use golem_common::model::plugin::PluginOwner;
use golem_common::model::{AccountId, HasAccountId, ProjectId};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, FromRepr};
use uuid::Uuid;

use crate::auth::CloudNamespace;
use crate::repo::CloudPluginOwnerRow;
use cloud_api_grpc::proto::golem::cloud::project::{Project, ProjectData};
use golem_common::newtype_uuid;

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

impl TryFrom<cloud_api_grpc::proto::golem::cloud::project::v1::GetProjectActionsSuccessResponse>
    for ProjectAuthorisedActions
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::project::v1::GetProjectActionsSuccessResponse,
    ) -> Result<Self, Self::Error> {
        let actions: HashSet<ProjectAction> = value
            .actions
            .into_iter()
            .map(|action| action.try_into())
            .collect::<Result<_, _>>()?;

        Ok(Self {
            project_id: value.project_id.ok_or("Missing worker_id")?.try_into()?,
            owner_account_id: value.owner_account_id.ok_or("Missing account_id")?.into(),
            actions: ProjectActions { actions },
        })
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
pub enum Role {
    Admin = 0,
    MarketingAdmin = 1,
    ViewProject = 2,
    DeleteProject = 3,
    CreateProject = 4,
    UpdateProject = 5,
    InstanceServer = 6,
    ViewPlugin = 7,
    CreatePlugin = 8,
    DeletePlugin = 9,
}

impl Role {
    pub fn all() -> Vec<Role> {
        Role::iter().collect::<Vec<Role>>()
    }

    pub fn all_project_roles() -> Vec<Role> {
        vec![
            Role::ViewProject,
            Role::DeleteProject,
            Role::CreateProject,
            Role::UpdateProject,
        ]
    }

    pub fn all_plugin_roles() -> Vec<Role> {
        vec![Role::ViewPlugin, Role::CreatePlugin, Role::DeletePlugin]
    }
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
            "ViewProject" => Ok(Role::ViewProject),
            "DeleteProject" => Ok(Role::DeleteProject),
            "CreateProject" => Ok(Role::CreateProject),
            "InstanceServer" => Ok(Role::InstanceServer),
            "ViewPlugin" => Ok(Role::ViewPlugin),
            "CreatePlugin" => Ok(Role::CreatePlugin),
            "DeletePlugin" => Ok(Role::DeletePlugin),
            "UpdateProject" => Ok(Role::UpdateProject),
            _ => Err(format!("Unknown role id: {}", s)),
        }
    }
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Role::Admin => write!(f, "Admin"),
            Role::MarketingAdmin => write!(f, "MarketingAdmin"),
            Role::ViewProject => write!(f, "ViewProject"),
            Role::DeleteProject => write!(f, "DeleteProject"),
            Role::CreateProject => write!(f, "CreateProject"),
            Role::InstanceServer => write!(f, "InstanceServer"),
            Role::ViewPlugin => write!(f, "ViewPlugin"),
            Role::CreatePlugin => write!(f, "CreatePlugin"),
            Role::DeletePlugin => write!(f, "DeletePlugin"),
            Role::UpdateProject => write!(f, "UpdateProject"),
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

impl HasAccountId for CloudPluginOwner {
    fn account_id(&self) -> AccountId {
        self.account_id.clone()
    }
}

impl PluginOwner for CloudPluginOwner {
    type Row = CloudPluginOwnerRow;
}
