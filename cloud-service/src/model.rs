use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::str::FromStr;

use chrono::{TimeZone, Utc};
use cloud_common::model::*;
use cloud_common::model::{PlanId, ProjectPolicyId, TokenId};
use golem_api_grpc::proto::golem::worker::Level;
use golem_common::model::{AccountId, ComponentVersion, ProjectId, Timestamp, WorkerStatus};
use golem_service_base::model::*;
use poem_openapi::{Enum, Object};
use serde_with::{serde_as, DurationSeconds};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct VersionInfo {
    pub version: String,
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
    ViewTemplate,
    CreateTemplate,
    UpdateTemplate,
    DeleteTemplate,
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
            ProjectAction::ViewTemplate => 0,
            ProjectAction::CreateTemplate => 1,
            ProjectAction::UpdateTemplate => 2,
            ProjectAction::DeleteTemplate => 3,
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
            0 => Ok(ProjectAction::ViewTemplate),
            1 => Ok(ProjectAction::CreateTemplate),
            2 => Ok(ProjectAction::UpdateTemplate),
            3 => Ok(ProjectAction::DeleteTemplate),
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
            ProjectAction::ViewTemplate => write!(f, "ViewTemplate"),
            ProjectAction::CreateTemplate => write!(f, "CreateTemplate"),
            ProjectAction::UpdateTemplate => write!(f, "UpdateTemplate"),
            ProjectAction::DeleteTemplate => write!(f, "DeleteTemplate"),
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

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum LimitsEndpointError {
    ArgValidationError { errors: Vec<String> },
    InternalError { error: String },
    Unauthorized { error: String },
    LimitExceeded { error: String },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub available_fuel: i64,
    pub max_memory_per_worker: i64,
}

impl From<ResourceLimits> for golem_api_grpc::proto::golem::common::ResourceLimits {
    fn from(value: ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::ResourceLimits> for ResourceLimits {
    fn from(value: golem_api_grpc::proto::golem::common::ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
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
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectPolicyData {
    pub name: String,
    pub project_actions: ProjectActions,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct TemplateQuery {
    pub project_id: Option<ProjectId>,
    pub template_name: TemplateName,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
// can't use enum here https://github.com/OpenAPITools/openapi-generator/issues/13257
pub struct ProjectGrantDataRequest {
    pub grantee_account_id: AccountId,
    pub project_policy_id: Option<ProjectPolicyId>,
    pub project_actions: Vec<ProjectAction>,
    pub project_policy_name: Option<String>,
}

impl From<ProjectGrantDataRequest>
    for cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantDataRequest
{
    fn from(value: ProjectGrantDataRequest) -> Self {
        Self {
            grantee_account_id: Some(value.grantee_account_id.into()),
            project_policy_id: value.project_policy_id.map(|v| v.into()),
            project_actions: value
                .project_actions
                .into_iter()
                .map(|action| action.into())
                .collect(),
            project_policy_name: value.project_policy_name.unwrap_or("".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct BatchUpdateResourceLimits {
    pub updates: HashMap<String, i64>,
}

impl From<BatchUpdateResourceLimits>
    for cloud_api_grpc::proto::golem::cloud::limit::BatchUpdateResourceLimits
{
    fn from(value: BatchUpdateResourceLimits) -> Self {
        Self {
            updates: value.updates,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Template {
    pub versioned_template_id: VersionedTemplateId,
    pub user_template_id: UserTemplateId,
    pub protected_template_id: ProtectedTemplateId,
    pub template_name: TemplateName,
    pub template_size: i32,
    pub metadata: TemplateMetadata,
    pub project_id: ProjectId,
}

impl TryFrom<golem_api_grpc::proto::golem::template::Template> for Template {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::template::Template,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_template_id: value
                .versioned_template_id
                .ok_or("Missing versioned_template_id")?
                .try_into()?,
            user_template_id: value
                .user_template_id
                .ok_or("Missing user_template_id")?
                .try_into()?,
            protected_template_id: value
                .protected_template_id
                .ok_or("Missing protected_template_id")?
                .try_into()?,
            template_name: TemplateName(value.template_name),
            template_size: value.template_size,
            metadata: value.metadata.ok_or("Missing metadata")?.try_into()?,
            project_id: value.project_id.ok_or("Missing project_id")?.try_into()?,
        })
    }
}

impl From<Template> for golem_api_grpc::proto::golem::template::Template {
    fn from(value: Template) -> Self {
        Self {
            versioned_template_id: Some(value.versioned_template_id.into()),
            user_template_id: Some(value.user_template_id.into()),
            protected_template_id: Some(value.protected_template_id.into()),
            template_name: value.template_name.0,
            template_size: value.template_size,
            metadata: Some(value.metadata.into()),
            project_id: Some(value.project_id.into()),
        }
    }
}

impl Template {
    pub fn next_version(self) -> Self {
        let new_version = VersionedTemplateId {
            template_id: self.versioned_template_id.template_id,
            version: self.versioned_template_id.version + 1,
        };
        Self {
            versioned_template_id: new_version.clone(),
            user_template_id: UserTemplateId {
                versioned_template_id: new_version.clone(),
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: new_version,
            },
            ..self
        }
    }
}

impl From<ProjectType> for i32 {
    fn from(value: ProjectType) -> Self {
        match value {
            ProjectType::Default => 0,
            ProjectType::NonDefault => 1,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Token {
    pub id: TokenId,
    pub account_id: AccountId,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl Token {
    pub fn admin() -> Self {
        Self {
            id: TokenId::try_from("0868571c-b6cc-4817-bae8-048cbcef91a0").unwrap(),
            account_id: AccountId {
                value: "admin".into(),
            },
            created_at: Utc::from_utc_datetime(&Utc, &chrono::NaiveDateTime::MIN),
            expires_at: Utc::from_utc_datetime(&Utc, &chrono::NaiveDateTime::MAX),
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::token::Token> for Token {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::token::Token,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or("Missing id")?.try_into()?,
            account_id: value.account_id.ok_or("Missing account_id")?.into(),
            created_at: chrono::DateTime::<chrono::Utc>::from_str(&value.created_at)
                .map_err(|err| format!("Invalid created_at value: {err}"))?,
            expires_at: chrono::DateTime::<chrono::Utc>::from_str(&value.expires_at)
                .map_err(|err| format!("Invalid expires_at value: {err}"))?,
        })
    }
}

impl From<Token> for cloud_api_grpc::proto::golem::cloud::token::Token {
    fn from(value: Token) -> Self {
        Self {
            id: Some(value.id.into()),
            account_id: Some(value.account_id.into()),
            created_at: value.created_at.to_rfc3339(),
            expires_at: value.expires_at.to_rfc3339(),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Enum,
)]
pub enum ProjectType {
    Default,
    NonDefault,
}

impl TryFrom<i32> for ProjectType {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ProjectType::Default),
            1 => Ok(ProjectType::NonDefault),
            _ => Err(format!("Invalid project type: {}", value)),
        }
    }
}

impl From<cloud_api_grpc::proto::golem::cloud::project::ProjectType> for ProjectType {
    fn from(value: cloud_api_grpc::proto::golem::cloud::project::ProjectType) -> Self {
        match value {
            cloud_api_grpc::proto::golem::cloud::project::ProjectType::Default => {
                ProjectType::Default
            }
            cloud_api_grpc::proto::golem::cloud::project::ProjectType::NonDefault => {
                ProjectType::NonDefault
            }
        }
    }
}

impl From<ProjectData> for cloud_api_grpc::proto::golem::cloud::project::ProjectData {
    fn from(value: ProjectData) -> Self {
        Self {
            name: value.name,
            owner_account_id: Some(value.owner_account_id.into()),
            description: value.description,
            default_environment_id: value.default_environment_id,
            project_type: value.project_type.into(),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectData {
    pub name: String,
    pub owner_account_id: AccountId,
    pub description: String,
    pub default_environment_id: String,
    pub project_type: ProjectType,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::project::ProjectData> for ProjectData {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::project::ProjectData,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            owner_account_id: value
                .owner_account_id
                .ok_or("Missing owner_account_id")?
                .into(),
            description: value.description,
            default_environment_id: value.default_environment_id,
            project_type: value.project_type.try_into()?,
        })
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Project {
    pub project_id: ProjectId,
    pub project_data: ProjectData,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::project::Project> for Project {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::project::Project,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            project_id: value.id.ok_or("Missing id")?.try_into()?,
            project_data: value.data.ok_or("Missing data")?.try_into()?,
        })
    }
}

impl From<Project> for cloud_api_grpc::proto::golem::cloud::project::Project {
    fn from(value: Project) -> Self {
        Self {
            id: Some(value.project_id.into()),
            data: Some(value.project_data.into()),
        }
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

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateTokenDTO {
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl From<CreateTokenDTO> for cloud_api_grpc::proto::golem::cloud::token::CreateTokenDto {
    fn from(value: CreateTokenDTO) -> Self {
        Self {
            expires_at: value.expires_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub account_id: AccountId,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub template_version: ComponentVersion,
    pub retry_count: u64,
    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerMetadata> for WorkerMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            account_id: value.account_id.ok_or("Missing account_id")?.into(),
            args: value.args,
            env: value.env,
            status: value.status.try_into()?,
            template_version: value.template_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value
                .updates
                .into_iter()
                .map(|update| update.try_into())
                .collect::<Result<Vec<UpdateRecord>, String>>()?,
            created_at: value.created_at.ok_or("Missing created_at")?.into(),
            last_error: value.last_error,
        })
    }
}

impl From<WorkerMetadata> for golem_api_grpc::proto::golem::worker::WorkerMetadata {
    fn from(value: WorkerMetadata) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            account_id: Some(value.account_id.into()),
            args: value.args,
            env: value.env,
            status: value.status.into(),
            template_version: value.template_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates.iter().cloned().map(|u| u.into()).collect(),
            created_at: Some(value.created_at.into()),
            last_error: value.last_error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<WorkerMetadata>,
    pub cursor: Option<u64>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectGrantData {
    pub grantee_account_id: AccountId,
    pub grantor_project_id: ProjectId,
    pub project_policy_id: ProjectPolicyId,
}

impl From<ProjectGrantData>
    for cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantData
{
    fn from(value: ProjectGrantData) -> Self {
        Self {
            grantee_account_id: Some(value.grantee_account_id.into()),
            grantor_project_id: Some(value.grantor_project_id.into()),
            project_policy_id: Some(value.project_policy_id.into()),
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantData>
    for ProjectGrantData
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantData,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            grantee_account_id: value
                .grantee_account_id
                .ok_or("Missing grantee_account_id")?
                .into(),
            grantor_project_id: value
                .grantor_project_id
                .ok_or("Missing grantor_project_id")?
                .try_into()?,
            project_policy_id: value
                .project_policy_id
                .ok_or("Missing project_policy_id")?
                .try_into()?,
        })
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct ProjectGrant {
    pub id: ProjectGrantId,
    pub data: ProjectGrantData,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrant> for ProjectGrant {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrant,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or("Missing id")?.try_into()?,
            data: value.data.ok_or("Missing data")?.try_into()?,
        })
    }
}

impl From<ProjectGrant> for cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrant {
    fn from(value: ProjectGrant) -> Self {
        Self {
            id: Some(value.id.into()),
            data: Some(value.data.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectPolicy {
    pub id: ProjectPolicyId,
    pub name: String,
    pub project_actions: ProjectActions,
}

impl From<ProjectPolicy> for cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectPolicy {
    fn from(value: ProjectPolicy) -> Self {
        Self {
            id: Some(value.id.into()),
            name: value.name,
            actions: Some(value.project_actions.into()),
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectPolicy> for ProjectPolicy {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectPolicy,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or("Missing id")?.try_into()?,
            name: value.name,
            project_actions: value.actions.ok_or("Missing actions")?.try_into()?,
        })
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PlanData {
    pub project_limit: i32,
    pub template_limit: i32,
    pub worker_limit: i32,
    pub storage_limit: i32,
    pub monthly_gas_limit: i64,
    pub monthly_upload_limit: i32,
}

impl Default for PlanData {
    fn default() -> Self {
        Self {
            project_limit: 100,
            template_limit: 100,
            worker_limit: 1000,
            storage_limit: 500000000,
            monthly_gas_limit: 1000000000000,
            monthly_upload_limit: 1000000000,
        }
    }
}

impl From<cloud_api_grpc::proto::golem::cloud::plan::PlanData> for PlanData {
    fn from(value: cloud_api_grpc::proto::golem::cloud::plan::PlanData) -> Self {
        Self {
            project_limit: value.project_limit,
            template_limit: value.template_limit,
            worker_limit: value.worker_limit,
            storage_limit: value.storage_limit,
            monthly_gas_limit: value.monthly_gas_limit,
            monthly_upload_limit: value.monthly_upload_limit,
        }
    }
}

impl From<PlanData> for cloud_api_grpc::proto::golem::cloud::plan::PlanData {
    fn from(value: PlanData) -> Self {
        Self {
            project_limit: value.project_limit,
            template_limit: value.template_limit,
            worker_limit: value.worker_limit,
            storage_limit: value.storage_limit,
            monthly_gas_limit: value.monthly_gas_limit,
            monthly_upload_limit: value.monthly_upload_limit,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Plan {
    pub plan_id: PlanId,
    pub plan_data: PlanData,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            plan_id: PlanId(Uuid::from_str("80b56370-1ed4-4d90-864b-e8809641995d").unwrap()),
            plan_data: Default::default(),
        }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::plan::Plan> for Plan {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::plan::Plan,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            plan_id: value.plan_id.ok_or("Missing field: plan_id")?.try_into()?,
            plan_data: value.plan_data.ok_or("Missing field: plan_data")?.into(),
        })
    }
}

impl From<Plan> for cloud_api_grpc::proto::golem::cloud::plan::Plan {
    fn from(value: Plan) -> Self {
        Self {
            plan_id: Some(value.plan_id.into()),
            plan_data: Some(value.plan_data.into()),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectDataRequest {
    pub name: String,
    pub owner_account_id: AccountId,
    pub description: String,
}

impl From<ProjectDataRequest> for cloud_api_grpc::proto::golem::cloud::project::ProjectDataRequest {
    fn from(value: ProjectDataRequest) -> Self {
        Self {
            name: value.name,
            owner_account_id: Some(value.owner_account_id.into()),
            description: value.description,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Account {
    pub id: AccountId,
    pub name: String,
    pub email: String,
    pub plan_id: PlanId,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::account::Account> for Account {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::account::Account,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or("Missing field: id")?.into(),
            name: value.name,
            email: value.email,
            plan_id: value.plan_id.ok_or("Missing field: plan_id")?.try_into()?,
        })
    }
}

impl From<Account> for cloud_api_grpc::proto::golem::cloud::account::Account {
    fn from(value: Account) -> Self {
        Self {
            id: Some(value.id.into()),
            name: value.name,
            email: value.email,
            plan_id: Some(value.plan_id.into()),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: AccountId,
    pub name: String,
    pub email: String,
    pub templates_count: i64,
    pub workers_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::accountsummary::AccountSummary>
    for AccountSummary
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::accountsummary::AccountSummary,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or("Missing field: id")?.into(),
            name: value.name,
            email: value.email,
            templates_count: value.template_count,
            workers_count: value.worker_count,
            created_at: chrono::DateTime::<chrono::Utc>::from_str(&value.created_at)
                .map_err(|err| format!("Invalid created_at value: {err}"))?,
        })
    }
}

impl From<AccountSummary> for cloud_api_grpc::proto::golem::cloud::accountsummary::AccountSummary {
    fn from(value: AccountSummary) -> Self {
        Self {
            id: Some(value.id.into()),
            name: value.name,
            email: value.email,
            template_count: value.templates_count,
            worker_count: value.workers_count,
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct AccountData {
    pub name: String,
    pub email: String,
}

impl From<AccountData> for cloud_api_grpc::proto::golem::cloud::account::AccountData {
    fn from(value: AccountData) -> Self {
        Self {
            name: value.name,
            email: value.email,
        }
    }
}

impl From<cloud_api_grpc::proto::golem::cloud::account::AccountData> for AccountData {
    fn from(value: cloud_api_grpc::proto::golem::cloud::account::AccountData) -> Self {
        Self {
            name: value.name,
            email: value.email,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct OAuth2Data {
    pub url: String,
    pub user_code: String,
    pub expires: chrono::DateTime<chrono::Utc>,
    pub encoded_session: String,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::login::OAuth2Data> for OAuth2Data {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::login::OAuth2Data,
    ) -> Result<Self, String> {
        Ok(Self {
            url: value.url,
            user_code: value.user_code,
            expires: chrono::DateTime::<chrono::Utc>::from_str(&value.expires)
                .map_err(|err| format!("Invalid expires value: {err}"))?,
            encoded_session: value.encoded_session,
        })
    }
}

impl From<OAuth2Data> for cloud_api_grpc::proto::golem::cloud::login::OAuth2Data {
    fn from(value: OAuth2Data) -> Self {
        Self {
            url: value.url,
            user_code: value.user_code,
            expires: value.expires.to_rfc3339(),
            encoded_session: value.encoded_session,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct UnsafeToken {
    pub data: Token,
    pub secret: TokenSecret,
}

impl UnsafeToken {
    pub fn new(data: Token, secret: TokenSecret) -> Self {
        Self { data, secret }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::token::UnsafeToken> for UnsafeToken {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::token::UnsafeToken,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            data: value.data.ok_or("Missing field: data")?.try_into()?,
            secret: value.secret.ok_or("Missing field: secret")?.try_into()?,
        })
    }
}

impl From<UnsafeToken> for cloud_api_grpc::proto::golem::cloud::token::UnsafeToken {
    fn from(value: UnsafeToken) -> Self {
        Self {
            data: Some(value.data.into()),
            secret: Some(value.secret.into()),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct DeleteAccountResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct DeleteGrantResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct HealthcheckResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct UpdateResourceLimitsResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct DeleteProjectResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct DeleteProjectGrantResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct DeleteTokenResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct SetEnabledResponse {}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Enum)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

impl From<Level> for LogLevel {
    fn from(value: Level) -> Self {
        match value {
            Level::Trace => LogLevel::Trace,
            Level::Debug => LogLevel::Debug,
            Level::Info => LogLevel::Info,
            Level::Warn => LogLevel::Warn,
            Level::Error => LogLevel::Error,
            Level::Critical => LogLevel::Critical,
        }
    }
}

impl TryFrom<i32> for LogLevel {
    type Error = String;

    fn try_from(value: i32) -> Result<LogLevel, String> {
        match value {
            0 => Ok(LogLevel::Trace),
            1 => Ok(LogLevel::Debug),
            2 => Ok(LogLevel::Info),
            3 => Ok(LogLevel::Warn),
            4 => Ok(LogLevel::Error),
            5 => Ok(LogLevel::Critical),
            _ => Err(format!("Invalid value for LogLevel: {}", value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LogEvent {
    StdOut(String),
    StdErr(String),
    Log {
        level: LogLevel,
        context: String,
        message: String,
    },
}

impl TryFrom<golem_api_grpc::proto::golem::worker::LogEvent> for LogEvent {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::LogEvent,
    ) -> Result<Self, Self::Error> {
        match value.event {
            Some(golem_api_grpc::proto::golem::worker::log_event::Event::Stdout(event)) => {
                Ok(LogEvent::StdOut(event.message))
            }
            Some(golem_api_grpc::proto::golem::worker::log_event::Event::Stderr(event)) => {
                Ok(LogEvent::StdErr(event.message))
            }
            Some(golem_api_grpc::proto::golem::worker::log_event::Event::Log(event)) => {
                Ok(LogEvent::Log {
                    level: event.level.try_into()?,
                    context: event.context,
                    message: event.message,
                })
            }
            None => Err("Missing field: event".to_string()),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum OAuth2Provider {
    Github,
}

impl Display for OAuth2Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuth2Provider::Github => write!(f, "github"),
        }
    }
}

impl FromStr for OAuth2Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(OAuth2Provider::Github),
            _ => Err(format!("Invalid OAuth2Provider: {s}")),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct OAuth2Token {
    pub provider: OAuth2Provider,
    pub external_id: String,
    pub account_id: AccountId,
    pub token_id: Option<TokenId>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OAuth2Session {
    pub device_code: String,
    #[serde_as(as = "DurationSeconds<f64>")]
    pub interval: std::time::Duration,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EncodedOAuth2Session {
    pub value: String,
}

pub struct OAuth2AccessToken {
    pub provider: OAuth2Provider,
    pub access_token: String,
}

#[derive(Clone, Debug)]
pub struct ExternalLogin {
    pub external_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub verified_emails: Vec<String>,
}
