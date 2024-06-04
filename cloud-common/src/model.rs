use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use derive_more::FromStr;
use golem_common::model::{AccountId, ProjectId};
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

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
)]
pub enum ProjectAction {
    ViewComponent,
    CreateComponent,
    UpdateComponent,
    DeleteComponent,
    ViewWorker,
    CreateWorker,
    UpdateWorker,
    DeleteWorker,
    ViewProjectGrants,
    CreateProjectGrants,
    DeleteProjectGrants,
    ViewApiDefinition,
    CreateApiDefinition,
    UpdateApiDefinition,
    DeleteApiDefinition,
}

impl From<ProjectAction> for i32 {
    fn from(value: ProjectAction) -> Self {
        match value {
            ProjectAction::ViewComponent => 0,
            ProjectAction::CreateComponent => 1,
            ProjectAction::UpdateComponent => 2,
            ProjectAction::DeleteComponent => 3,
            ProjectAction::ViewWorker => 4,
            ProjectAction::CreateWorker => 5,
            ProjectAction::UpdateWorker => 6,
            ProjectAction::DeleteWorker => 7,
            ProjectAction::ViewProjectGrants => 8,
            ProjectAction::CreateProjectGrants => 9,
            ProjectAction::DeleteProjectGrants => 10,
            ProjectAction::ViewApiDefinition => 11,
            ProjectAction::CreateApiDefinition => 12,
            ProjectAction::UpdateApiDefinition => 13,
            ProjectAction::DeleteApiDefinition => 14,
        }
    }
}

impl TryFrom<i32> for ProjectAction {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ProjectAction::ViewComponent),
            1 => Ok(ProjectAction::CreateComponent),
            2 => Ok(ProjectAction::UpdateComponent),
            3 => Ok(ProjectAction::DeleteComponent),
            4 => Ok(ProjectAction::ViewWorker),
            5 => Ok(ProjectAction::CreateWorker),
            6 => Ok(ProjectAction::UpdateWorker),
            7 => Ok(ProjectAction::DeleteWorker),
            8 => Ok(ProjectAction::ViewProjectGrants),
            9 => Ok(ProjectAction::CreateProjectGrants),
            10 => Ok(ProjectAction::DeleteProjectGrants),
            11 => Ok(ProjectAction::ViewApiDefinition),
            12 => Ok(ProjectAction::CreateApiDefinition),
            13 => Ok(ProjectAction::UpdateApiDefinition),
            14 => Ok(ProjectAction::DeleteApiDefinition),
            _ => Err(format!("Invalid project action: {}", value)),
        }
    }
}

impl std::fmt::Display for ProjectAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

impl TryFrom<cloud_api_grpc::proto::golem::cloud::project::GetProjectActionsSuccessResponse>
    for ProjectAuthorisedActions
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::project::GetProjectActionsSuccessResponse,
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
)]
pub enum Role {
    Admin,
    MarketingAdmin,
    ViewProject,
    DeleteProject,
    CreateProject,
    InstanceServer,
}

impl Role {
    pub fn all() -> Vec<Role> {
        Role::iter().collect::<Vec<Role>>()
    }

    pub fn all_project_roles() -> Vec<Role> {
        vec![Role::ViewProject, Role::DeleteProject, Role::CreateProject]
    }
}

impl From<Role> for i32 {
    fn from(value: Role) -> Self {
        match value {
            Role::Admin => 0,
            Role::MarketingAdmin => 1,
            Role::ViewProject => 2,
            Role::DeleteProject => 3,
            Role::CreateProject => 4,
            Role::InstanceServer => 5,
        }
    }
}

impl TryFrom<i32> for Role {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Role::Admin),
            1 => Ok(Role::MarketingAdmin),
            2 => Ok(Role::ViewProject),
            3 => Ok(Role::DeleteProject),
            4 => Ok(Role::CreateProject),
            5 => Ok(Role::InstanceServer),
            _ => Err(format!("Invalid role: {}", value)),
        }
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
            _ => Err(format!("Unknown role id: {}", s)),
        }
    }
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Role::Admin => write!(f, "Admin"),
            Role::MarketingAdmin => write!(f, "MarketingAdmin"),
            Role::ViewProject => write!(f, "ViewProject"),
            Role::DeleteProject => write!(f, "DeleteProject"),
            Role::CreateProject => write!(f, "CreateProject"),
            Role::InstanceServer => write!(f, "InstanceServer"),
        }
    }
}
