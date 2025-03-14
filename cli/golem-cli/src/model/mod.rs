// Copyright 2024-2025 Golem Cloud
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

pub mod app_ext;
pub mod app_ext_raw;
pub mod component;
pub mod deploy;
pub mod invoke_result_view;
pub mod plugin_cloud;
pub mod plugin_manifest;
pub mod project;
pub mod text;
pub mod to_cli;
pub mod to_cloud;
pub mod to_oss;
pub mod wave;

use crate::cloud::{AccountId, ProjectId};
use crate::config::{CloudProfile, NamedProfile, OssProfile, Profile, ProfileConfig, ProfileName};
use crate::model::to_oss::ToOss;
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, Error, ValueEnum};
use clap_verbosity_flag::Verbosity;
use golem_client::model::{ApiDefinitionInfo, ApiSite, Provider};
use golem_common::model::trim_date::TrimDateTime;
use golem_templates::model::{GuestLanguage, GuestLanguageTier, Template, TemplateName};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;
use uuid::Uuid;

// TODO: move arg thing into command
// TODO: move non generic entities into mods

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

pub trait HasFormatConfig {
    fn format(&self) -> Option<Format>;
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ProjectName(pub String);

impl From<&str> for ProjectName {
    fn from(name: &str) -> Self {
        ProjectName(name.to_string())
    }
}

impl From<String> for ProjectName {
    fn from(name: String) -> Self {
        ProjectName(name)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ComponentName(pub String);

impl From<&str> for ComponentName {
    fn from(name: &str) -> Self {
        ComponentName(name.to_string())
    }
}

impl From<String> for ComponentName {
    fn from(name: String) -> Self {
        ComponentName(name)
    }
}

impl Display for ComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WorkerName(pub String);

impl From<&str> for WorkerName {
    fn from(name: &str) -> Self {
        WorkerName(name.to_string())
    }
}

impl From<String> for WorkerName {
    fn from(name: String) -> Self {
        WorkerName(name)
    }
}

impl Display for WorkerName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct IdempotencyKey(pub String);

impl Default for IdempotencyKey {
    fn default() -> Self {
        Self::new()
    }
}

impl IdempotencyKey {
    pub fn new() -> Self {
        IdempotencyKey(Uuid::new_v4().to_string())
    }
}

impl From<&str> for IdempotencyKey {
    fn from(value: &str) -> Self {
        IdempotencyKey(value.to_string())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdentityProviderType {
    Google,
    Facebook,
    Gitlab,
    Microsoft,
}

impl Display for IdentityProviderType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Google => "google",
            Self::Facebook => "facebook",
            Self::Gitlab => "gitlab",
            Self::Microsoft => "microsoft",
        };
        Display::fmt(&s, f)
    }
}

impl FromStr for IdentityProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "google" => Ok(IdentityProviderType::Google),
            "facebook" => Ok(IdentityProviderType::Facebook),
            "gitlab" => Ok(IdentityProviderType::Gitlab),
            "microsoft" => Ok(IdentityProviderType::Microsoft),
            _ => Err(format!(
                "Unknown identity provider type: {s}. Expected one of \"google\", \"facebook\", \"gitlab\", \"microsoft\""
            )),
        }
    }
}

impl From<IdentityProviderType> for Provider {
    fn from(value: IdentityProviderType) -> Self {
        match value {
            IdentityProviderType::Google => Provider::Google,
            IdentityProviderType::Facebook => Provider::Facebook,
            IdentityProviderType::Gitlab => Provider::Gitlab,
            IdentityProviderType::Microsoft => Provider::Microsoft,
        }
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ApiDefinitionId(pub String);

impl Display for ApiDefinitionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ApiDefinitionId {
    fn from(id: &str) -> Self {
        ApiDefinitionId(id.to_string())
    }
}

impl From<String> for ApiDefinitionId {
    fn from(id: String) -> Self {
        ApiDefinitionId(id)
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ApiDefinitionFileFormat {
    Json,
    Yaml,
}

impl Display for ApiDefinitionFileFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        };
        Display::fmt(&s, f)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ApiDefinitionVersion(pub String);

impl Display for ApiDefinitionVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ApiDefinitionVersion {
    fn from(id: &str) -> Self {
        ApiDefinitionVersion(id.to_string())
    }
}

impl From<String> for ApiDefinitionVersion {
    fn from(id: String) -> Self {
        ApiDefinitionVersion(id)
    }
}

#[derive(Clone)]
pub struct JsonValueParser;

impl TypedValueParser for JsonValueParser {
    type Value = Value;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, Error> {
        let inner = StringValueParser::new();
        let val = inner.parse_ref(cmd, arg, value)?;
        let parsed = <Value as FromStr>::from_str(&val);

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
pub struct TemplateDescription {
    pub name: TemplateName,
    pub language: GuestLanguage,
    pub tier: GuestLanguageTier,
    pub description: String,
}

impl TemplateDescription {
    pub fn from_template(template: &Template) -> Self {
        Self {
            name: template.name.clone(),
            language: template.language,
            description: template.description.clone(),
            tier: template.language.tier(),
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

#[derive(Clone, PartialEq, Eq, Debug)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerMetadataView {
    pub component_name: ComponentName,
    pub worker_name: WorkerName,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub account_id: Option<AccountId>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: golem_client::model::WorkerStatus,
    pub component_version: u64,
    pub retry_count: u64,

    pub pending_invocation_count: u64,
    pub updates: Vec<golem_client::model::UpdateRecord>,
    pub created_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<String, golem_client::model::ResourceMetadata>,
}

impl TrimDateTime for WorkerMetadataView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            created_at: self.created_at.trim_date_time_ms(),
            ..self
        }
    }
}

impl From<WorkerMetadata> for WorkerMetadataView {
    fn from(value: WorkerMetadata) -> Self {
        WorkerMetadataView {
            component_name: value.component_name,
            worker_name: value.worker_id.worker_name.into(),
            account_id: value.account_id,
            args: value.args,
            env: value.env,
            status: value.status,
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates,
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value.owned_resources,
        }
    }
}

// TODO: active plugins?
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerMetadata {
    pub worker_id: golem_client::model::WorkerId,
    pub component_name: ComponentName,
    pub account_id: Option<AccountId>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: golem_client::model::WorkerStatus,
    pub component_version: u64,
    pub retry_count: u64,
    pub pending_invocation_count: u64,
    pub updates: Vec<golem_client::model::UpdateRecord>,
    pub created_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<String, golem_client::model::ResourceMetadata>,
}

impl WorkerMetadata {
    pub fn from_oss(
        component_name: ComponentName,
        value: golem_client::model::WorkerMetadata,
    ) -> Self {
        WorkerMetadata {
            worker_id: value.worker_id,
            component_name,
            account_id: None,
            args: value.args,
            env: value.env,
            status: value.status,
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates,
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value.owned_resources,
        }
    }

    pub fn from_cloud(
        component_name: ComponentName,
        value: golem_cloud_client::model::WorkerMetadata,
    ) -> Self {
        WorkerMetadata {
            worker_id: value.worker_id.to_oss(),
            component_name,
            account_id: Some(AccountId(value.account_id)),
            args: value.args,
            env: value.env,
            status: value.status.to_oss(),
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates.to_oss(),
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources: value.owned_resources.to_oss(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkersMetadataResponseView {
    pub workers: Vec<WorkerMetadataView>,
    pub cursors: BTreeMap<String, String>,
}

impl TrimDateTime for WorkersMetadataResponseView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            workers: self.workers.trim_date_time_ms(),
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiDeployment {
    #[serde(rename = "apiDefinitions")]
    pub api_definitions: Vec<ApiDefinitionInfo>,
    #[serde(rename = "projectId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub project_id: Option<Uuid>,
    pub site: ApiSite,
    #[serde(rename = "createdAt")]
    pub created_at: Option<DateTime<Utc>>,
}

impl From<golem_client::model::ApiDeployment> for ApiDeployment {
    fn from(value: golem_client::model::ApiDeployment) -> Self {
        ApiDeployment {
            api_definitions: value.api_definitions,
            project_id: None,
            site: value.site,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiSecurityScheme {
    #[serde(rename = "schemeIdentifier")]
    pub scheme_identifier: String,
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "clientSecret")]
    pub client_secret: String,
    #[serde(rename = "redirectUrl")]
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

impl From<golem_client::model::SecuritySchemeData> for ApiSecurityScheme {
    fn from(value: golem_client::model::SecuritySchemeData) -> Self {
        ApiSecurityScheme {
            scheme_identifier: value.scheme_identifier,
            client_id: value.client_id,
            client_secret: value.client_secret,
            redirect_url: value.redirect_url,
            scopes: value.scopes,
        }
    }
}

pub trait HasVerbosity {
    fn verbosity(&self) -> Verbosity;
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct OssPluginScopeArgs {
    /// Global scope (plugin available for all components)
    #[arg(long, group = "plugin-scope-args")]
    global: bool,

    /// Component scope given by a component URN or URL (plugin only available for this component)
    #[arg(long, short = 'C', value_name = "URI", group = "plugin-scope-args")]
    component: Option<ComponentName>,

    /// Component scope given by the component's name (plugin only available for this component)
    #[arg(long, short = 'c', group = "plugin-scope-args")]
    component_name: Option<String>,
}

pub fn decode_api_definition<'de, T: Deserialize<'de>>(
    input: &'de str,
    format: &ApiDefinitionFileFormat,
) -> anyhow::Result<T> {
    match format {
        ApiDefinitionFileFormat::Json => serde_json::from_str(input)
            .map_err(|e| anyhow!("Failed to parse json api definition: {e:?}")),
        ApiDefinitionFileFormat::Yaml => serde_yaml::from_str(input)
            .map_err(|e| anyhow!("Failed to parse yaml api definition: {e:?}")),
    }
}

#[derive(Debug, Clone)]
pub struct WorkerConnectOptions {
    pub colors: bool,
    pub show_timestamp: bool,
    pub show_level: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProfileView {
    pub is_active: bool,
    pub name: ProfileName,
    pub typ: ProfileType,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cloud_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_url: Option<Url>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub allow_insecure: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticated: Option<bool>,
    pub config: ProfileConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum ProfileType {
    Golem,
    GolemCloud,
}

impl ProfileView {
    pub fn from_profile(active: &ProfileName, profile: NamedProfile) -> Self {
        let NamedProfile { name, profile } = profile;

        match profile {
            Profile::Golem(OssProfile {
                url,
                worker_url,
                allow_insecure,
                config,
            }) => ProfileView {
                is_active: &name == active,
                name,
                typ: ProfileType::Golem,
                url: Some(url),
                cloud_url: None,
                worker_url,
                allow_insecure,
                authenticated: None,
                config,
            },
            Profile::GolemCloud(CloudProfile {
                custom_url,
                custom_cloud_url,
                custom_worker_url,
                allow_insecure,
                auth,
                config,
            }) => ProfileView {
                is_active: &name == active,
                name,
                typ: ProfileType::GolemCloud,
                url: custom_url,
                cloud_url: custom_cloud_url,
                worker_url: custom_worker_url,
                allow_insecure,
                authenticated: Some(auth.is_some()),
                config,
            },
        }
    }
}

pub struct ProjectNameAndId {
    pub project_name: ProjectName,
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentNameMatchKind {
    AppCurrentDir,
    App,
    Unknown,
}

pub struct WorkerNameMatch {
    pub project: Option<ProjectNameAndId>,
    pub component_name_match_kind: ComponentNameMatchKind,
    pub component_name: ComponentName,
    pub worker_name: Option<WorkerName>,
}

pub struct SelectedComponents {
    pub account_id: Option<AccountId>,
    pub project: Option<ProjectNameAndId>,
    pub component_names: Vec<ComponentName>,
}
