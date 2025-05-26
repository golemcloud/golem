use chrono::{TimeZone, Utc};
use cloud_common::model::*;
use cloud_common::model::{PlanId, ProjectPolicyId, TokenId};
use golem_api_grpc::proto::golem::worker::Level;
use golem_common::model::plugin::PluginInstallationTarget;
use golem_common::model::{AccountId, ProjectId};
use golem_service_base::model::*;
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct VersionInfo {
    pub version: String,
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
pub struct ComponentQuery {
    pub project_id: Option<ProjectId>,
    pub component_name: ComponentName,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
// can't use enum here https://github.com/OpenAPITools/openapi-generator/issues/13257
pub struct ProjectGrantDataRequest {
    pub grantee_account_id: Option<AccountId>,
    pub grantee_email: Option<String>,
    pub project_policy_id: Option<ProjectPolicyId>,
    pub project_actions: Vec<ProjectPermisison>,
    pub project_policy_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct BatchUpdateResourceLimits {
    pub updates: HashMap<String, i64>,
}

impl From<BatchUpdateResourceLimits>
    for cloud_api_grpc::proto::golem::cloud::limit::v1::BatchUpdateResourceLimits
{
    fn from(value: BatchUpdateResourceLimits) -> Self {
        Self {
            updates: value.updates,
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
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
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
            created_at: chrono::DateTime::<Utc>::from_str(&value.created_at)
                .map_err(|err| format!("Invalid created_at value: {err}"))?,
            expires_at: chrono::DateTime::<Utc>::from_str(&value.expires_at)
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
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateTokenDTO {
    pub expires_at: chrono::DateTime<Utc>,
}

impl From<CreateTokenDTO> for cloud_api_grpc::proto::golem::cloud::token::CreateTokenDto {
    fn from(value: CreateTokenDTO) -> Self {
        Self {
            expires_at: value.expires_at.to_rfc3339(),
        }
    }
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

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct ProjectGrant {
    pub id: ProjectGrantId,
    pub data: ProjectGrantData,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectPolicy {
    pub id: ProjectPolicyId,
    pub name: String,
    pub project_actions: ProjectActions,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PlanData {
    pub project_limit: i32,
    pub component_limit: i32,
    pub worker_limit: i32,
    pub storage_limit: i32,
    pub monthly_gas_limit: i64,
    pub monthly_upload_limit: i32,
}

impl Default for PlanData {
    fn default() -> Self {
        Self {
            project_limit: 100,
            component_limit: 100,
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
            component_limit: value.component_limit,
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
            component_limit: value.component_limit,
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
    pub component_count: i64,
    pub worker_count: i64,
    pub created_at: chrono::DateTime<Utc>,
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::accountsummary::v1::AccountSummary>
    for AccountSummary
{
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::accountsummary::v1::AccountSummary,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or("Missing field: id")?.into(),
            name: value.name,
            email: value.email,
            component_count: value.component_count,
            worker_count: value.worker_count,
            created_at: chrono::DateTime::<Utc>::from_str(&value.created_at)
                .map_err(|err| format!("Invalid created_at value: {err}"))?,
        })
    }
}

impl From<AccountSummary>
    for cloud_api_grpc::proto::golem::cloud::accountsummary::v1::AccountSummary
{
    fn from(value: AccountSummary) -> Self {
        Self {
            id: Some(value.id.into()),
            name: value.name,
            email: value.email,
            component_count: value.component_count,
            worker_count: value.worker_count,
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
    pub expires: chrono::DateTime<Utc>,
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
            expires: chrono::DateTime::<Utc>::from_str(&value.expires)
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
    pub expires_at: chrono::DateTime<Utc>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProjectPluginInstallationTarget {
    pub project_id: ProjectId,
}

impl Display for ProjectPluginInstallationTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.project_id)
    }
}

impl PluginInstallationTarget for ProjectPluginInstallationTarget {
    type Row = crate::repo::plugin_installation::ProjectPluginInstallationTargetRow;

    fn table_name() -> &'static str {
        "project_plugin_installation"
    }
}

#[derive(Debug, Clone)]
pub enum GlobalAction {
    CreateAccount,
    ViewAccountSummaries,
    ViewAccountCount,
}

#[derive(Debug, Clone)]
pub enum AccountAction {
    ViewAccount,
    UpdateAccount,
    ViewPlan,
    CreateProject,
    DeleteAccount,
    ViewAccountGrants,
    CreateAccountGrant,
    DeleteAccountGrant,
    ViewDefaultProject,
    ListProjectGrants,
    ViewLimits,
    UpdateLimits,
}
