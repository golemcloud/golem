use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

use crate::service::auth::CloudNamespace;
use bincode::{Decode, Encode};
use cloud_api_grpc::proto::golem::cloud::project::{Project, ProjectData};
use derive_more::{Display, FromStr};
use golem_common::model::{AccountId, ComponentId, ScanCursor};
use golem_common::model::{ComponentVersion, ProjectId, Timestamp, WorkerStatus};
use golem_service_base::model::{ResourceMetadata, UpdateRecord, WorkerId};
use golem_worker_service_base::api_definition::http::HttpApiDefinition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiSite, ApiVersion};
use poem_openapi::{NewType, Object};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

pub enum GolemResult {
    Ok(Box<dyn PrintRes>),
    Json(serde_json::value::Value),
    Str(String),
}

pub trait PrintRes {
    fn println(&self, format: &Format);
}

impl<T> PrintRes for T
where
    T: Serialize,
{
    fn println(&self, format: &Format) {
        match format {
            Format::Json => println!("{}", serde_json::to_string_pretty(self).unwrap()),
            Format::Yaml => println!("{}", serde_yaml::to_string(self).unwrap()),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
pub enum Format {
    Json,
    Yaml,
}

impl Display for Format {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        };
        Display::fmt(&s, f)
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Format::Json),
            "yaml" => Ok(Format::Yaml),
            _ => {
                let all = Format::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown format: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProjectRef {
    Id(ProjectId),
    Name(String),
    Default,
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

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Encode, Decode)]
pub struct ComponentName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ComponentIdOrName {
    Id(ComponentId),
    Name(ComponentName, ProjectRef),
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct WorkerName(pub String); // TODO: Validate

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub account_id: AccountId,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub component_version: ComponentVersion,
    pub retry_count: u64,
    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<u64, ResourceMetadata>,
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
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value
                .updates
                .into_iter()
                .map(|update| update.try_into())
                .collect::<Result<Vec<UpdateRecord>, String>>()?,
            created_at: value.created_at.ok_or("Missing created_at")?.into(),
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value
                .owned_resources
                .into_iter()
                .map(|(k, v)| v.try_into().map(|v| (k, v)))
                .collect::<Result<HashMap<_, _>, _>>()?,
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
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates.iter().cloned().map(|u| u.into()).collect(),
            created_at: Some(value.created_at.into()),
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value
                .owned_resources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<WorkerMetadata>,
    pub cursor: Option<ScanCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDeployment {
    pub api_definitions: Vec<ApiDefinitionInfo>,
    pub project_id: ProjectId,
    pub site: ApiSite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDefinitionInfo {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

impl From<golem_worker_service_base::api_definition::ApiDeployment<CloudNamespace>>
    for ApiDeployment
{
    fn from(
        api_deployment: golem_worker_service_base::api_definition::ApiDeployment<CloudNamespace>,
    ) -> Self {
        Self {
            api_definitions: api_deployment
                .api_definition_keys
                .iter()
                .map(|k| ApiDefinitionInfo {
                    id: k.id.clone(),
                    version: k.version.clone(),
                })
                .collect(),
            project_id: api_deployment.namespace.project_id.clone(),
            site: api_deployment.site.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct AccountApiDeployment {
    pub account_id: AccountId,
    pub deployment: ApiDeployment,
}

impl AccountApiDeployment {
    pub fn new(account_id: &AccountId, deployment: &ApiDeployment) -> Self {
        Self {
            account_id: account_id.clone(),
            deployment: deployment.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct AccountApiDefinition {
    pub account_id: AccountId,
    pub definition: HttpApiDefinition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDomain {
    pub project_id: ProjectId,
    pub domain_name: String,
    pub name_servers: Vec<String>,
}

impl ApiDomain {
    pub fn new(request: &DomainRequest, name_servers: Vec<String>) -> Self {
        Self {
            project_id: request.project_id.clone(),
            domain_name: request.domain_name.clone(),
            name_servers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct AccountApiDomain {
    pub account_id: AccountId,
    pub domain: ApiDomain,
}

impl AccountApiDomain {
    pub fn new(account_id: &AccountId, domain: &ApiDomain) -> Self {
        Self {
            account_id: account_id.clone(),
            domain: domain.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct DomainRequest {
    pub project_id: ProjectId,
    pub domain_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CertificateRequest {
    pub project_id: ProjectId,
    pub domain_name: String,
    pub certificate_body: String,
    pub certificate_private_key: String,
}

#[derive(
    Debug, Clone, Eq, PartialEq, Hash, FromStr, Serialize, Deserialize, Encode, Decode, NewType,
)]
pub struct CertificateId(#[bincode(with_serde)] pub Uuid);

impl Display for CertificateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Certificate {
    pub id: CertificateId,
    pub project_id: ProjectId,
    pub domain_name: String,
}

impl Certificate {
    pub fn new(request: &CertificateRequest) -> Self {
        Self {
            id: CertificateId(Uuid::new_v4()),
            project_id: request.project_id.clone(),
            domain_name: request.domain_name.clone(),
        }
    }
}
