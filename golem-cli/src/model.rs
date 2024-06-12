// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod component;
pub mod invoke_result_view;
pub mod text;
pub mod wave;

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;

use crate::cloud::model::AccountId;
use crate::model::text::TextFormat;
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, ArgMatches, Command, Error, FromArgMatches};
use derive_more::{Display, FromStr};
use golem_client::model::{ApiDefinitionInfo, ApiSite, ScanCursor};
use golem_examples::model::{Example, ExampleName, GuestLanguage, GuestLanguageTier};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

pub enum GolemResult {
    Ok(Box<dyn PrintRes>),
    Json(serde_json::value::Value),
    Str(String),
}

impl GolemResult {
    pub fn err(s: String) -> Result<GolemResult, GolemError> {
        Err(GolemError(s))
    }

    pub fn print(self, format: Format) {
        match self {
            GolemResult::Ok(r) => r.println(&format),
            GolemResult::Str(s) => println!("{s}"),
            GolemResult::Json(json) => match format {
                Format::Json | Format::Text => {
                    println!("{}", serde_json::to_string_pretty(&json).unwrap())
                }
                Format::Yaml => println!("{}", serde_yaml::to_string(&json).unwrap()),
            },
        }
    }
}

pub trait PrintRes {
    fn println(&self, format: &Format);
}

impl<T> PrintRes for T
where
    T: Serialize,
    T: TextFormat,
{
    fn println(&self, format: &Format) {
        match format {
            Format::Json => println!("{}", serde_json::to_string_pretty(self).unwrap()),
            Format::Yaml => println!("{}", serde_yaml::to_string(self).unwrap()),
            Format::Text => self.print(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GolemError(pub String);

impl From<reqwest::Error> for GolemError {
    fn from(error: reqwest::Error) -> Self {
        GolemError(format!("Unexpected client error: {error}"))
    }
}

impl From<reqwest::header::InvalidHeaderValue> for GolemError {
    fn from(value: reqwest::header::InvalidHeaderValue) -> Self {
        GolemError(format!("Invalid request header: {value}"))
    }
}

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl<T: ResponseContentErrorMapper> From<golem_client::Error<T>> for GolemError {
    fn from(value: golem_client::Error<T>) -> Self {
        match value {
            golem_client::Error::Reqwest(error) => GolemError::from(error),
            golem_client::Error::ReqwestHeader(invalid_header) => GolemError::from(invalid_header),
            golem_client::Error::Serde(error) => {
                GolemError(format!("Unexpected serialization error: {error}"))
            }
            golem_client::Error::Item(data) => {
                let error_str = ResponseContentErrorMapper::map(data);
                GolemError(error_str)
            }
            golem_client::Error::Unexpected { code, data } => {
                match String::from_utf8(Vec::from(data)) {
                    Ok(data_string) => GolemError(format!(
                        "Unexpected http error. Code: {code}, content: {data_string}."
                    )),
                    Err(_) => GolemError(format!(
                        "Unexpected http error. Code: {code}, can't parse content as string."
                    )),
                }
            }
        }
    }
}

impl<T: ResponseContentErrorMapper> From<golem_cloud_client::Error<T>> for GolemError {
    fn from(value: golem_cloud_client::Error<T>) -> Self {
        match value {
            golem_cloud_client::Error::Reqwest(error) => GolemError::from(error),
            golem_cloud_client::Error::ReqwestHeader(invalid_header) => {
                GolemError::from(invalid_header)
            }
            golem_cloud_client::Error::Serde(error) => {
                GolemError(format!("Unexpected serialization error: {error}"))
            }
            golem_cloud_client::Error::Item(data) => {
                let error_str = ResponseContentErrorMapper::map(data);
                GolemError(error_str)
            }
            golem_cloud_client::Error::Unexpected { code, data } => {
                match String::from_utf8(Vec::from(data)) {
                    Ok(data_string) => GolemError(format!(
                        "Unexpected http error. Code: {code}, content: {data_string}."
                    )),
                    Err(_) => GolemError(format!(
                        "Unexpected http error. Code: {code}, can't parse content as string."
                    )),
                }
            }
        }
    }
}

impl Display for GolemError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let GolemError(s) = self;
        Display::fmt(s, f)
    }
}

impl Debug for GolemError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let GolemError(s) = self;
        Display::fmt(s, f)
    }
}

impl std::error::Error for GolemError {
    fn description(&self) -> &str {
        let GolemError(s) = self;

        s
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize, Default)]
pub enum Format {
    Json,
    Yaml,
    #[default]
    Text,
}

impl Display for Format {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Text => "text",
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
            "text" => Ok(Format::Text),
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

impl FromArgMatches for ComponentIdOrName {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ComponentIdOrNameArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: ComponentIdOrNameArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = ComponentIdOrNameArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for ComponentIdOrName {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ComponentIdOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ComponentIdOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct ComponentIdOrNameArgs {
    #[arg(short = 'C', long, conflicts_with = "component_name", required = true)]
    component_id: Option<Uuid>,

    #[arg(short, long, conflicts_with = "component_id", required = true)]
    component_name: Option<String>,
}

impl From<&ComponentIdOrNameArgs> for ComponentIdOrName {
    fn from(value: &ComponentIdOrNameArgs) -> ComponentIdOrName {
        if let Some(id) = value.component_id {
            ComponentIdOrName::Id(ComponentId(id))
        } else {
            ComponentIdOrName::Name(ComponentName(
                value.component_name.as_ref().unwrap().to_string(),
            ))
        }
    }
}

impl From<&ComponentIdOrName> for ComponentIdOrNameArgs {
    fn from(value: &ComponentIdOrName) -> ComponentIdOrNameArgs {
        match value {
            ComponentIdOrName::Id(ComponentId(id)) => ComponentIdOrNameArgs {
                component_id: Some(*id),
                component_name: None,
            },
            ComponentIdOrName::Name(ComponentName(name)) => ComponentIdOrNameArgs {
                component_id: None,
                component_name: Some(name.clone()),
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ComponentName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ComponentIdOrName {
    Id(ComponentId),
    Name(ComponentName),
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct WorkerName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Serialize, Deserialize)]
pub struct IdempotencyKey(pub String); // TODO: Validate

impl IdempotencyKey {
    pub fn fresh() -> Self {
        IdempotencyKey(Uuid::new_v4().to_string())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ApiDefinitionIdWithVersion {
    pub id: ApiDefinitionId,
    pub version: ApiDefinitionVersion,
}

impl Display for ApiDefinitionIdWithVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.id, self.version)
    }
}

impl FromStr for ApiDefinitionIdWithVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid api definition id with version: {s}. Expected format: <id>/<version>"
            ));
        }

        let id = ApiDefinitionId(parts[0].to_string());
        let version = ApiDefinitionVersion(parts[1].to_string());

        Ok(ApiDefinitionIdWithVersion { id, version })
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ApiDefinitionId(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ApiDefinitionVersion(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ComponentId(pub Uuid);

#[derive(Clone)]
pub struct JsonValueParser;

impl TypedValueParser for JsonValueParser {
    type Value = serde_json::value::Value;

    fn parse_ref(
        &self,
        cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, Error> {
        let inner = StringValueParser::new();
        let val = inner.parse_ref(cmd, arg, value)?;
        let parsed = <serde_json::Value as std::str::FromStr>::from_str(&val);

        match parsed {
            Ok(value) => Ok(value),
            Err(serde_err) => {
                let mut err = clap::Error::new(ErrorKind::ValueValidation);
                if let Some(arg) = arg {
                    err.insert(
                        ContextKind::InvalidArg,
                        ContextValue::String(arg.to_string()),
                    );
                }
                err.insert(
                    ContextKind::InvalidValue,
                    ContextValue::String(format!("Invalid JSON value: {serde_err}")),
                );
                Err(err)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct ExampleDescription {
    pub name: ExampleName,
    pub language: GuestLanguage,
    pub tier: GuestLanguageTier,
    pub description: String,
}

impl ExampleDescription {
    pub fn from_example(example: &Example) -> Self {
        Self {
            name: example.name.clone(),
            language: example.language.clone(),
            description: example.description.clone(),
            tier: example.language.tier(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum PathBufOrStdin {
    Path(PathBuf),
    Stdin,
}

impl FromStr for PathBufOrStdin {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "-" {
            Ok(PathBufOrStdin::Stdin)
        } else {
            Ok(PathBufOrStdin::Path(PathBuf::from_str(s)?))
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display)]
pub enum WorkerUpdateMode {
    Automatic,
    Manual,
}

impl FromStr for WorkerUpdateMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(WorkerUpdateMode::Automatic),
            "manual" => Ok(WorkerUpdateMode::Manual),
            _ => Err(format!(
                "Unknown mode: {s}. Expected one of \"auto\", \"manual\""
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerMetadata {
    #[serde(rename = "workerId")]
    pub worker_id: golem_client::model::WorkerId,
    #[serde(rename = "accountId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub account_id: Option<AccountId>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: golem_client::model::WorkerStatus,
    #[serde(rename = "componentVersion")]
    pub component_version: u64,
    #[serde(rename = "retryCount")]
    pub retry_count: u64,
    #[serde(rename = "pendingInvocationCount")]
    pub pending_invocation_count: u64,
    pub updates: Vec<golem_client::model::UpdateRecord>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "lastError")]
    pub last_error: Option<String>,
}

impl From<golem_client::model::WorkerMetadata> for WorkerMetadata {
    fn from(value: golem_client::model::WorkerMetadata) -> Self {
        let golem_client::model::WorkerMetadata {
            worker_id,
            args,
            env,
            status,
            component_version,
            retry_count,
            pending_invocation_count,
            updates,
            created_at,
            last_error,
        } = value;

        WorkerMetadata {
            worker_id,
            account_id: None,
            args,
            env,
            status,
            component_version,
            retry_count,
            pending_invocation_count,
            updates,
            created_at,
            last_error,
        }
    }
}

pub fn to_oss_worker_id(id: golem_cloud_client::model::WorkerId) -> golem_client::model::WorkerId {
    golem_client::model::WorkerId {
        component_id: id.component_id,
        worker_name: id.worker_name,
    }
}

pub fn to_oss_worker_status(
    s: golem_cloud_client::model::WorkerStatus,
) -> golem_client::model::WorkerStatus {
    match s {
        golem_cloud_client::model::WorkerStatus::Running => {
            golem_client::model::WorkerStatus::Running
        }
        golem_cloud_client::model::WorkerStatus::Idle => golem_client::model::WorkerStatus::Idle,
        golem_cloud_client::model::WorkerStatus::Suspended => {
            golem_client::model::WorkerStatus::Suspended
        }
        golem_cloud_client::model::WorkerStatus::Interrupted => {
            golem_client::model::WorkerStatus::Interrupted
        }
        golem_cloud_client::model::WorkerStatus::Retrying => {
            golem_client::model::WorkerStatus::Retrying
        }
        golem_cloud_client::model::WorkerStatus::Failed => {
            golem_client::model::WorkerStatus::Failed
        }
        golem_cloud_client::model::WorkerStatus::Exited => {
            golem_client::model::WorkerStatus::Exited
        }
    }
}

fn to_oss_update_record(
    r: golem_cloud_client::model::UpdateRecord,
) -> golem_client::model::UpdateRecord {
    fn to_oss_pending_update(
        u: golem_cloud_client::model::PendingUpdate,
    ) -> golem_client::model::PendingUpdate {
        golem_client::model::PendingUpdate {
            timestamp: u.timestamp,
            target_version: u.target_version,
        }
    }
    fn to_oss_successful_update(
        u: golem_cloud_client::model::SuccessfulUpdate,
    ) -> golem_client::model::SuccessfulUpdate {
        golem_client::model::SuccessfulUpdate {
            timestamp: u.timestamp,
            target_version: u.target_version,
        }
    }
    fn to_oss_failed_update(
        u: golem_cloud_client::model::FailedUpdate,
    ) -> golem_client::model::FailedUpdate {
        golem_client::model::FailedUpdate {
            timestamp: u.timestamp,
            target_version: u.target_version,
            details: u.details,
        }
    }

    match r {
        golem_cloud_client::model::UpdateRecord::PendingUpdate(pu) => {
            golem_client::model::UpdateRecord::PendingUpdate(to_oss_pending_update(pu))
        }
        golem_cloud_client::model::UpdateRecord::SuccessfulUpdate(su) => {
            golem_client::model::UpdateRecord::SuccessfulUpdate(to_oss_successful_update(su))
        }
        golem_cloud_client::model::UpdateRecord::FailedUpdate(fu) => {
            golem_client::model::UpdateRecord::FailedUpdate(to_oss_failed_update(fu))
        }
    }
}

impl From<golem_cloud_client::model::WorkerMetadata> for WorkerMetadata {
    fn from(value: golem_cloud_client::model::WorkerMetadata) -> Self {
        let golem_cloud_client::model::WorkerMetadata {
            worker_id,
            account_id,
            args,
            env,
            status,
            component_version,
            retry_count,
            pending_invocation_count,
            updates,
            created_at,
            last_error,
        } = value;

        WorkerMetadata {
            worker_id: to_oss_worker_id(worker_id),
            account_id: Some(AccountId::new(account_id)),
            args,
            env,
            status: to_oss_worker_status(status),
            component_version,
            retry_count,
            pending_invocation_count,
            updates: updates.into_iter().map(to_oss_update_record).collect(),
            created_at,
            last_error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<WorkerMetadata>,
    pub cursor: Option<ScanCursor>,
}

impl From<golem_client::model::WorkersMetadataResponse> for WorkersMetadataResponse {
    fn from(value: golem_client::model::WorkersMetadataResponse) -> Self {
        WorkersMetadataResponse {
            cursor: value.cursor,
            workers: value.workers.into_iter().map(|m| m.into()).collect(),
        }
    }
}

impl From<golem_cloud_client::model::WorkersMetadataResponse> for WorkersMetadataResponse {
    fn from(value: golem_cloud_client::model::WorkersMetadataResponse) -> Self {
        WorkersMetadataResponse {
            cursor: value.cursor.map(|c| golem_client::model::ScanCursor {
                cursor: c,
                layer: 0,
            }), // TODO: unify cloud and OSS
            workers: value.workers.into_iter().map(|m| m.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDeployment {
    #[serde(rename = "apiDefinitions")]
    pub api_definitions: Vec<ApiDefinitionInfo>,
    #[serde(rename = "projectId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub project_id: Option<Uuid>,
    pub site: ApiSite,
}

impl From<golem_client::model::ApiDeployment> for ApiDeployment {
    fn from(value: golem_client::model::ApiDeployment) -> Self {
        ApiDeployment {
            api_definitions: value.api_definitions,
            project_id: None,
            site: value.site,
        }
    }
}

impl From<golem_cloud_client::model::ApiDeployment> for ApiDeployment {
    fn from(value: golem_cloud_client::model::ApiDeployment) -> Self {
        let golem_cloud_client::model::ApiDeployment {
            api_definition_id,
            version,
            project_id,
            site: golem_cloud_client::model::ApiSite { host, subdomain },
        } = value;

        let api_definitions = vec![ApiDefinitionInfo {
            id: api_definition_id,
            version,
        }];

        ApiDeployment {
            api_definitions,
            project_id: Some(project_id),
            site: ApiSite {
                host,
                subdomain: Some(subdomain),
            },
        }
    }
}
