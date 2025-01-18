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
pub mod plugin_manifest;
pub mod text;
pub mod wave;

use crate::cloud::AccountId;
use crate::command::{ComponentRefSplit, ComponentRefsSplit};
use crate::model::text::fmt::TextFormat;
use crate::oss::model::OssContext;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, ArgMatches, Error, FromArgMatches, ValueEnum};
use clap_verbosity_flag::Verbosity;
use golem_client::model::{ApiDefinitionInfo, ApiSite, Provider, ScanCursor};
use golem_common::model::plugin::{ComponentPluginScope, DefaultPluginScope};
use golem_common::model::trim_date::TrimDateTime;
use golem_common::model::{ComponentId, Empty};
use golem_common::uri::oss::uri::ComponentUri;
use golem_common::uri::oss::url::ComponentUrl;
use golem_common::uri::oss::urn::WorkerUrn;
use golem_examples::model::{Example, ExampleName, GuestLanguage, GuestLanguageTier};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

pub enum GolemResult {
    Ok(Box<dyn PrintRes>),
    Json(Value),
    Str(String),
    Empty,
}

impl GolemResult {
    pub fn err(s: String) -> Result<GolemResult, GolemError> {
        Err(GolemError(s))
    }
}

impl PrintRes for GolemResult {
    fn println(&self, format: Format) {
        match self {
            GolemResult::Ok(value) => value.println(format),
            GolemResult::Json(json) => match format {
                Format::Json | Format::Text => {
                    println!("{}", serde_json::to_string_pretty(&json).unwrap())
                }
                Format::Yaml => println!("{}", serde_yaml::to_string(&json).unwrap()),
            },
            GolemResult::Str(string) => println!("{}", string),
            GolemResult::Empty => {
                // NOP
            }
        }
    }

    fn streaming_print(&self, format: Format) {
        match self {
            GolemResult::Ok(value) => value.streaming_print(format),
            GolemResult::Json(json) => match format {
                Format::Json | Format::Text => {
                    println!("{}", serde_json::to_string(&json).unwrap())
                }
                Format::Yaml => println!("---\n{}", serde_yaml::to_string(&json).unwrap()),
            },
            GolemResult::Str(string) => println!("{}", string),
            GolemResult::Empty => (), // NOP
        }
    }
}

pub trait PrintRes {
    fn println(&self, format: Format);
    fn streaming_print(&self, format: Format);
}

impl<T> PrintRes for T
where
    T: Serialize,
    T: TextFormat,
{
    fn println(&self, format: Format) {
        match format {
            Format::Json => println!("{}", serde_json::to_string_pretty(self).unwrap()),
            Format::Yaml => println!("{}", serde_yaml::to_string(self).unwrap()),
            Format::Text => self.print(),
        }
    }

    fn streaming_print(&self, format: Format) {
        match format {
            Format::Json => println!("{}", serde_json::to_string(self).unwrap()),
            Format::Yaml => println!("---\n{}", serde_yaml::to_string(self).unwrap()),
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

impl From<anyhow::Error> for GolemError {
    fn from(value: anyhow::Error) -> Self {
        GolemError(format!("{value:#}"))
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

pub trait HasFormatConfig {
    fn format(&self) -> Option<Format>;
}

impl FromArgMatches for ComponentUriArg {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ComponentUriOrNameArgs::from_arg_matches(matches).map(Into::into)
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let mut args: ComponentUriOrNameArgs = self.clone().into();
        ComponentUriOrNameArgs::update_from_arg_matches(&mut args, matches).map(|()| {
            *self = args.into();
        })
    }
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = true, multiple = false)]
struct ComponentUriOrNameArgs {
    /// Component URI. Either URN or URL.
    #[arg(
        short = 'C',
        long,
        group = "component_group",
        required = true,
        value_name = "URI"
    )]
    component: Option<ComponentUri>,

    /// Name of the component
    #[arg(short, long, group = "component_group", required = true)]
    component_name: Option<String>,
}

impl From<ComponentUriOrNameArgs> for ComponentUriArg {
    fn from(value: ComponentUriOrNameArgs) -> ComponentUriArg {
        if let Some(uri) = value.component {
            ComponentUriArg {
                uri,
                explicit_name: false,
            }
        } else {
            ComponentUriArg {
                uri: ComponentUri::URL(ComponentUrl {
                    name: value.component_name.unwrap(),
                }),
                explicit_name: true,
            }
        }
    }
}

impl From<ComponentUriArg> for ComponentUriOrNameArgs {
    fn from(value: ComponentUriArg) -> ComponentUriOrNameArgs {
        let name = match &value.uri {
            ComponentUri::URL(url) if value.explicit_name => Some(&url.name),
            _ => None,
        };

        match name {
            None => ComponentUriOrNameArgs {
                component: Some(value.uri.clone()),
                component_name: None,
            },
            Some(name) => ComponentUriOrNameArgs {
                component: None,
                component_name: Some(name.to_string()),
            },
        }
    }
}

impl From<ComponentUriOrNamesArgs> for ComponentUrisArg {
    fn from(value: ComponentUriOrNamesArgs) -> ComponentUrisArg {
        if let Some(uri) = value.component {
            ComponentUrisArg {
                uris: vec![uri],
                explicit_name: false,
            }
        } else {
            ComponentUrisArg {
                uris: value
                    .component_name
                    .into_iter()
                    .map(|component_name| {
                        ComponentUri::URL(ComponentUrl {
                            name: component_name,
                        })
                    })
                    .collect(),
                explicit_name: true,
            }
        }
    }
}

impl From<ComponentUrisArg> for ComponentUriOrNamesArgs {
    fn from(mut value: ComponentUrisArg) -> ComponentUriOrNamesArgs {
        if value.explicit_name {
            ComponentUriOrNamesArgs {
                component: None,
                component_name: value
                    .uris
                    .into_iter()
                    .map(|uri| match uri {
                        ComponentUri::URN(_) => {
                            panic!("Unexpected URN")
                        }
                        ComponentUri::URL(url) => url.name,
                    })
                    .collect(),
            }
        } else {
            if value.uris.len() != 1 {
                panic!("Expected exactly one URI");
            }
            ComponentUriOrNamesArgs {
                component: Some(value.uris.swap_remove(0)),
                component_name: vec![],
            }
        }
    }
}

impl FromArgMatches for ComponentUrisArg {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ComponentUriOrNamesArgs::from_arg_matches(matches).map(Into::into)
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let mut args: ComponentUriOrNamesArgs = self.clone().into();
        ComponentUriOrNamesArgs::update_from_arg_matches(&mut args, matches).map(|()| {
            *self = args.into();
        })
    }
}

#[derive(clap::Args, Debug, Clone)]
struct ComponentUriOrNamesArgs {
    /// Component URI. Either URN or URL.
    #[arg(
        short = 'C',
        long,
        value_name = "URI",
        conflicts_with_all = vec!["component_name"],
    )]
    component: Option<ComponentUri>,

    /// Name of the component(s). When used with application manifest then multiple ones can be defined.
    #[arg(short, long)]
    component_name: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, derive_more::Display, derive_more::FromStr)]
pub struct ComponentName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ComponentUriArg {
    pub uri: ComponentUri,
    pub explicit_name: bool,
}

impl ComponentRefSplit<OssContext> for ComponentUriArg {
    fn split(self) -> (ComponentUri, Option<OssContext>) {
        (self.uri, None)
    }
}

impl clap::Args for ComponentUriArg {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ComponentUriOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ComponentUriOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ComponentUrisArg {
    pub uris: Vec<ComponentUri>,
    pub explicit_name: bool,
}

impl ComponentRefsSplit<OssContext> for ComponentUrisArg {
    fn split(self) -> Option<(Vec<ComponentUri>, Option<OssContext>)> {
        Some((self.uris, None))
    }
}

impl clap::Args for ComponentUrisArg {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ComponentUriOrNamesArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ComponentUriOrNamesArgs::augment_args_for_update(cmd)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, derive_more::Display, derive_more::FromStr)]
pub struct WorkerName(pub String); // TODO: Validate

#[derive(
    Clone, PartialEq, Eq, Debug, derive_more::Display, derive_more::FromStr, Serialize, Deserialize,
)]
pub struct IdempotencyKey(pub String); // TODO: Validate

impl IdempotencyKey {
    pub fn fresh() -> Self {
        IdempotencyKey(Uuid::new_v4().to_string())
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

#[derive(Clone, PartialEq, Eq, Debug, derive_more::Display, derive_more::FromStr)]
pub struct ApiDefinitionId(pub String); // TODO: Validate

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

#[derive(Clone, PartialEq, Eq, Debug, derive_more::Display, derive_more::FromStr)]
pub struct ApiDefinitionVersion(pub String); // TODO: Validate

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
            language: example.language,
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

#[derive(Clone, PartialEq, Eq, Debug, derive_more::Display)]
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
pub struct WorkerMetadataView {
    #[serde(rename = "workerUrn")]
    pub worker_urn: WorkerUrn,
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
    #[serde(rename = "componentSize")]
    pub component_size: u64,
    #[serde(rename = "totalLinearMemorySize")]
    pub total_linear_memory_size: u64,
    #[serde(rename = "ownedResources")]
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
        let WorkerMetadata {
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
            component_size,
            total_linear_memory_size,
            owned_resources,
        } = value;

        WorkerMetadataView {
            worker_urn: WorkerUrn {
                id: worker_id.into_target_worker_id(),
            },
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
            component_size,
            total_linear_memory_size,
            owned_resources,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerMetadata {
    pub worker_id: golem_client::model::WorkerId,
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
            component_size,
            total_linear_memory_size,
            owned_resources,
            active_plugins: _active_plugins,
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
            component_size,
            total_linear_memory_size,
            owned_resources,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkersMetadataResponseView {
    pub workers: Vec<WorkerMetadataView>,
    pub cursor: Option<ScanCursor>,
}

impl TrimDateTime for WorkersMetadataResponseView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            workers: self.workers.trim_date_time_ms(),
            ..self
        }
    }
}

impl From<WorkersMetadataResponse> for WorkersMetadataResponseView {
    fn from(value: WorkersMetadataResponse) -> Self {
        let WorkersMetadataResponse { workers, cursor } = value;

        WorkersMetadataResponseView {
            workers: workers.into_iter().map_into().collect(),
            cursor,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
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

#[async_trait]
pub trait ComponentIdResolver<ComponentRef> {
    async fn resolve(&self, component: ComponentRef) -> Result<ComponentId, GolemError>;
}

/// Represents a CLI argument data type describing a plugin scope
#[async_trait]
pub trait PluginScopeArgs {
    type PluginScope;
    type ComponentRef;

    async fn into(
        self,
        resolver: impl ComponentIdResolver<Self::ComponentRef> + Send,
    ) -> Result<Option<Self::PluginScope>, GolemError>;
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct OssPluginScopeArgs {
    /// Global scope (plugin available for all components)
    #[arg(long, group = "plugin-scope-args")]
    global: bool,

    /// Component scope given by a component URN or URL (plugin only available for this component)
    #[arg(long, short = 'C', value_name = "URI", group = "plugin-scope-args")]
    component: Option<ComponentUri>,

    /// Component scope given by the component's name (plugin only available for this component)
    #[arg(long, short = 'c', group = "plugin-scope-args")]
    component_name: Option<String>,
}

#[async_trait]
impl PluginScopeArgs for OssPluginScopeArgs {
    type PluginScope = DefaultPluginScope;
    type ComponentRef = ComponentUriArg;

    async fn into(
        self,
        resolver: impl ComponentIdResolver<ComponentUriArg> + Send,
    ) -> Result<Option<DefaultPluginScope>, GolemError> {
        if self.global {
            Ok(Some(DefaultPluginScope::Global(Empty {})))
        } else if let Some(uri) = self.component {
            let component_id = resolver
                .resolve(ComponentUriArg {
                    uri,
                    explicit_name: false,
                })
                .await?;
            Ok(Some(DefaultPluginScope::Component(ComponentPluginScope {
                component_id,
            })))
        } else if let Some(name) = self.component_name {
            let component_id = resolver
                .resolve(ComponentUriArg {
                    uri: ComponentUri::URL(ComponentUrl { name }),
                    explicit_name: true,
                })
                .await?;
            Ok(Some(DefaultPluginScope::Component(ComponentPluginScope {
                component_id,
            })))
        } else {
            Ok(None)
        }
    }
}

pub fn decode_api_definition<'de, T: Deserialize<'de>>(
    input: &'de str,
    format: &ApiDefinitionFileFormat,
) -> Result<T, GolemError> {
    match format {
        ApiDefinitionFileFormat::Json => serde_json::from_str(input)
            .map_err(|e| GolemError(format!("Failed to parse json api definition: {e:?}"))),
        ApiDefinitionFileFormat::Yaml => serde_yaml::from_str(input)
            .map_err(|e| GolemError(format!("Failed to parse yaml api definition: {e:?}"))),
    }
}
