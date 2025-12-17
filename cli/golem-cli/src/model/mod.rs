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
pub mod app;
pub mod app_raw;
pub mod cascade;
pub mod component;
pub mod deploy;
pub mod environment;
pub mod format;
pub mod http_api;
pub mod invoke_result_view;
// TODO: atomic: pub mod plugin_manifest;
pub mod template;
pub mod text;
pub mod wave;
pub mod worker;

use crate::command::shared_args::ComponentTemplateName;
use crate::config::AuthenticationConfig;
use crate::config::{NamedProfile, ProfileConfig, ProfileName};
use crate::log::LogColorize;
use anyhow::{anyhow, Context};
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, Error};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_templates::model::{GuestLanguage, GuestLanguageTier, Template, TemplateName};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::io::Read;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;
// TODO: move non generic entities into mods

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

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProfileView {
    pub is_active: bool,
    pub name: ProfileName,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
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
            worker_url: profile.custom_worker_url,
            allow_insecure: profile.allow_insecure,
            authenticated,
            config: profile.config,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountDetails {
    pub account_id: AccountId,
    pub email: String,
}

impl From<golem_client::model::Account> for AccountDetails {
    fn from(value: golem_client::model::Account) -> Self {
        Self {
            account_id: value.id,
            email: value.email.0,
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

// TODO: atomic
/*
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
*/

pub struct NewInteractiveApp {
    pub app_name: ApplicationName,
    pub templated_component_names: Vec<(ComponentTemplateName, ComponentName)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenApiDefinitionOutputFormat {
    Json,
    Yaml,
}

impl FromStr for OpenApiDefinitionOutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(OpenApiDefinitionOutputFormat::Json),
            "yaml" | "yml" => Ok(OpenApiDefinitionOutputFormat::Yaml),
            _ => Err(format!(
                "Invalid API definition format: {s}. Expected one of \"json\", \"yaml\""
            )),
        }
    }
}

impl Display for OpenApiDefinitionOutputFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenApiDefinitionOutputFormat::Json => write!(f, "json"),
            OpenApiDefinitionOutputFormat::Yaml => write!(f, "yaml"),
        }
    }
}
