// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod agent;
pub mod api;
pub mod app;
pub mod app_raw;
pub mod component;
pub mod deploy;
pub mod deploy_diff;
pub mod invoke_result_view;
pub mod plugin_manifest;
pub mod project;
pub mod template;
pub mod text;
pub mod wave;
pub mod worker;

use crate::command::shared_args::{ComponentTemplateName, StreamArgs};
use crate::config::AuthenticationConfig;
use crate::config::{NamedProfile, ProfileConfig, ProfileName};
use crate::log::LogColorize;
use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, Error};
use clap_verbosity_flag::Verbosity;
use colored::control::SHOULD_COLORIZE;
use golem_client::model::PluginTypeSpecificDefinition;
use golem_common::model::trim_date::TrimDateTime;
use golem_common::model::{
    AgentInstanceDescription, AgentInstanceKey, ExportedResourceInstanceDescription,
};
use golem_templates::model::{
    GuestLanguage, GuestLanguageTier, PackageName, Template, TemplateName,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::io::Read;
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;
use uuid::Uuid;
// TODO: move arg thing into command
// TODO: move non generic entities into mods

// NOTE: using aliases for lower-case support in manifest, as global configs are using the
//       PascalCase versions historically, should be cleared up (migrated), if we touch the global
//       CLI config
#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize, Default)]
pub enum Format {
    #[serde(alias = "json")]
    Json,
    #[serde(alias = "yaml")]
    Yaml,
    #[default]
    #[serde(alias = "text")]
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PluginReference {
    RelativeToCurrentAccount {
        name: String,
        version: String,
    },
    FullyQualified {
        account_email: String,
        name: String,
        version: String,
    },
}

impl PluginReference {
    pub fn account_email(&self) -> Option<String> {
        match self {
            Self::FullyQualified { account_email, .. } => Some(account_email.clone()),
            Self::RelativeToCurrentAccount { .. } => None,
        }
    }

    pub fn plugin_name(&self) -> String {
        match self {
            Self::FullyQualified { name, .. } => name.clone(),
            Self::RelativeToCurrentAccount { name, .. } => name.clone(),
        }
    }

    pub fn plugin_version(&self) -> String {
        match self {
            Self::FullyQualified { version, .. } => version.clone(),
            Self::RelativeToCurrentAccount { version, .. } => version.clone(),
        }
    }
}

impl FromStr for PluginReference {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut segments = s.split("/").collect::<Vec<_>>();
        match segments.len() {
            2 => {
                let version = segments.pop().unwrap().to_string();
                let name = segments.pop().unwrap().to_string();
                Ok(Self::RelativeToCurrentAccount { name, version })
            }
            3 => {
                let version = segments.pop().unwrap().to_string();
                let name = segments.pop().unwrap().to_string();
                let account_email = segments.pop().unwrap().to_string();
                Ok(Self::FullyQualified { account_email, name, version })
            }
            _ => Err(format!("Unknown format for plugin: {}. Expected either <PLUGIN_NAME>/<PLUGIN_VERSION> or <ACCOUNT_EMAIL>/<PLUGIN_NAME>/<PLUGIN_VERSION>", s.log_color_highlight()))
        }
    }
}

impl Display for PluginReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RelativeToCurrentAccount { name, version } => write!(f, "{name}/{version}"),
            Self::FullyQualified {
                account_email,
                name,
                version,
            } => write!(f, "{account_email}/{name}/{version}"),
        }
    }
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProjectReference {
    JustName(ProjectName),
    WithAccount {
        account_email: String,
        project_name: ProjectName,
    },
}

impl FromStr for ProjectReference {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut segments = s.split("/").collect::<Vec<_>>();
        match segments.len() {
            1 => Ok(Self::JustName(segments.pop().unwrap().into())),
            2 => {
                let project_name = segments.pop().unwrap().into();
                let account_email = segments.pop().unwrap().to_string();
                Ok(Self::WithAccount { account_email, project_name })
            }
            _ => Err(format!("Unknown format for project: {}. Expected either <PROJECT_NAME> or <ACCOUNT_EMAIL>/<PROJECT_NAME>", s.log_color_highlight()))
        }
    }
}

impl Display for ProjectReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JustName(project_name) => write!(f, "{}", project_name.0),
            Self::WithAccount {
                account_email,
                project_name,
            } => write!(f, "{}/{}", account_email, project_name.0),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
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

pub enum ComponentVersionSelection<'a> {
    ByWorkerName(&'a WorkerName),
    ByExplicitVersion(u64),
}

impl<'a> From<&'a WorkerName> for ComponentVersionSelection<'a> {
    fn from(value: &'a WorkerName) -> Self {
        Self::ByWorkerName(value)
    }
}

impl From<u64> for ComponentVersionSelection<'_> {
    fn from(value: u64) -> Self {
        Self::ByExplicitVersion(value)
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

impl PathBufOrStdin {
    pub fn read_to_string(&self) -> anyhow::Result<String> {
        match self {
            PathBufOrStdin::Path(path) => std::fs::read_to_string(path)
                .with_context(|| anyhow!("Failed to read file: {}", path.display())),
            PathBufOrStdin::Stdin => {
                let mut content = String::new();
                let _ = std::io::stdin()
                    .read_to_string(&mut content)
                    .with_context(|| anyhow!("Failed to read from STDIN"))?;
                Ok(content)
            }
        }
    }

    pub fn is_stdin(&self) -> bool {
        match self {
            PathBufOrStdin::Path(_) => false,
            PathBufOrStdin::Stdin => true,
        }
    }
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorkerUpdateMode {
    Automatic,
    Manual,
}

impl Display for WorkerUpdateMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerUpdateMode::Automatic => {
                write!(f, "auto")
            }
            WorkerUpdateMode::Manual => {
                write!(f, "manual")
            }
        }
    }
}

impl FromStr for WorkerUpdateMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(WorkerUpdateMode::Automatic),
            "manual" => Ok(WorkerUpdateMode::Manual),
            _ => Err(format!(
                "Unknown worker update mode: {s}. Expected one of \"auto\", \"manual\""
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
    pub created_by: Option<AccountId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub project_id: Option<ProjectId>,
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
    pub exported_resource_instances: HashMap<String, ExportedResourceInstanceDescription>,
    pub agent_instances: HashMap<AgentInstanceKey, AgentInstanceDescription>,
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
            created_by: value.created_by,
            project_id: value.project_id,
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
            exported_resource_instances: value.exported_resource_instances,
            agent_instances: value.agent_instances,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerMetadata {
    pub worker_id: golem_client::model::WorkerId,
    pub component_name: ComponentName,
    pub project_id: Option<ProjectId>,
    pub created_by: Option<AccountId>,
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
    pub exported_resource_instances: HashMap<String, ExportedResourceInstanceDescription>,
    pub agent_instances: HashMap<AgentInstanceKey, AgentInstanceDescription>,
}

impl WorkerMetadata {
    pub fn from(component_name: ComponentName, value: golem_client::model::WorkerMetadata) -> Self {
        WorkerMetadata {
            worker_id: value.worker_id,
            component_name,
            created_by: None,
            project_id: None,
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
            exported_resource_instances: HashMap::from_iter(
                value.exported_resource_instances.into_iter().map(|desc| {
                    let key = desc.key.resource_id.to_string();
                    (key, desc.description)
                }),
            ),
            agent_instances: HashMap::from_iter(
                value
                    .agent_instances
                    .into_iter()
                    .map(|desc| (desc.key, desc.description)),
            ),
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

pub trait HasVerbosity {
    fn verbosity(&self) -> Verbosity;
}

#[derive(Debug, Clone)]
pub struct WorkerConnectOptions {
    pub colors: bool,
    pub show_timestamp: bool,
    pub show_level: bool,
}

impl From<StreamArgs> for WorkerConnectOptions {
    fn from(args: StreamArgs) -> Self {
        WorkerConnectOptions {
            colors: SHOULD_COLORIZE.should_colorize(),
            show_timestamp: !args.stream_no_timestamp,
            show_level: !args.stream_no_log_level,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProfileView {
    pub is_active: bool,
    pub name: ProfileName,
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

impl ProfileView {
    pub fn from_profile(active: &ProfileName, profile: NamedProfile) -> Self {
        let NamedProfile { name, profile } = profile;

        let authenticated = match &profile.auth {
            AuthenticationConfig::OAuth2(inner) => Some(inner.data.is_some()),
            AuthenticationConfig::Static(_) => None,
        };

        ProfileView {
            is_active: &name == active,
            name,
            url: profile.custom_url,
            cloud_url: profile.custom_cloud_url,
            worker_url: profile.custom_worker_url,
            allow_insecure: profile.allow_insecure,
            authenticated,
            config: profile.config,
        }
    }
}

pub struct ProjectRefAndId {
    pub project_ref: ProjectReference,
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentNameMatchKind {
    AppCurrentDir,
    App,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct AccountDetails {
    pub account_id: AccountId,
    pub email: String,
}

impl From<golem_client::model::Account> for AccountDetails {
    fn from(value: golem_client::model::Account) -> Self {
        Self {
            account_id: value.id.into(),
            email: value.email,
        }
    }
}

pub struct WorkerNameMatch {
    pub account: Option<AccountDetails>,
    pub project: Option<ProjectRefAndId>,
    pub component_name_match_kind: ComponentNameMatchKind,
    pub component_name: ComponentName,
    pub worker_name: Option<WorkerName>,
}

pub struct SelectedComponents {
    pub account: Option<AccountDetails>,
    pub project: Option<ProjectRefAndId>,
    pub component_names: Vec<ComponentName>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TokenId(pub Uuid);

impl FromStr for TokenId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(TokenId(Uuid::parse_str(s)?))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProjectPolicyId(pub Uuid);

impl FromStr for ProjectPolicyId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(ProjectPolicyId(Uuid::parse_str(s)?))
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize)]
pub enum Role {
    Admin,
    MarketingAdmin,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Admin => "Admin",
            Role::MarketingAdmin => "MarketingAdmin",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            _ => {
                let all = Role::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown role: {s}. Expected one of {all}"))
            }
        }
    }
}

impl From<Role> for golem_client::model::Role {
    fn from(value: Role) -> Self {
        match value {
            Role::Admin => golem_client::model::Role::Admin,
            Role::MarketingAdmin => golem_client::model::Role::MarketingAdmin,
        }
    }
}

impl From<golem_client::model::Role> for Role {
    fn from(value: golem_client::model::Role) -> Self {
        match value {
            golem_client::model::Role::Admin => Role::Admin,
            golem_client::model::Role::MarketingAdmin => Role::MarketingAdmin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinition {
    pub name: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub scope: String,
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_transformer_validate_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_transformer_transform_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oplog_processor_component_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oplog_processor_component_version: Option<u64>,
}

impl From<golem_client::model::PluginDefinition> for PluginDefinition {
    fn from(value: golem_client::model::PluginDefinition) -> Self {
        let mut plugin_definition = Self {
            name: value.name,
            version: value.version,
            description: value.description,
            homepage: value.homepage,
            scope: value.scope.to_string(),
            typ: "".to_string(),
            component_transformer_validate_url: None,
            component_transformer_transform_url: None,
            oplog_processor_component_id: None,
            oplog_processor_component_version: None,
        };

        match value.specs {
            PluginTypeSpecificDefinition::ComponentTransformer(specs) => {
                plugin_definition.typ = "Component Transformer".to_string();
                plugin_definition.component_transformer_validate_url = Some(specs.validate_url);
                plugin_definition.component_transformer_transform_url = Some(specs.transform_url);
            }
            PluginTypeSpecificDefinition::OplogProcessor(specs) => {
                plugin_definition.typ = "Oplog Processor".to_string();
                plugin_definition.oplog_processor_component_id =
                    Some(specs.component_id.to_string());
                plugin_definition.oplog_processor_component_version = Some(specs.component_version);
            }
            PluginTypeSpecificDefinition::Library(_) => {
                plugin_definition.typ = "Library".to_string();
            }
            PluginTypeSpecificDefinition::App(_) => plugin_definition.typ = "App".to_string(),
        };

        plugin_definition
    }
}

pub struct NewInteractiveApp {
    pub app_name: String,
    pub templated_component_names: Vec<(ComponentTemplateName, PackageName)>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AccountId(pub String);

impl From<String> for AccountId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for AccountId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

impl From<Uuid> for ProjectId {
    fn from(uuid: Uuid) -> Self {
        ProjectId(uuid)
    }
}

impl From<ProjectId> for Uuid {
    fn from(project_id: ProjectId) -> Self {
        project_id.0
    }
}

impl Display for ProjectId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
