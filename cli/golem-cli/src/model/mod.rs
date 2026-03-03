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
pub mod cli_command_metadata;
pub mod component;
pub mod deploy;
pub mod environment;
pub mod format;
pub mod http_api;
pub mod invoke_result_view;
pub mod plugin;
pub mod plugin_manifest;
pub mod repl;
pub mod template;
pub mod text;
pub mod wave;
pub mod worker;

use crate::app::template::{AppTemplate, AppTemplateName};
use crate::command::shared_args::ComponentTemplateName;
use crate::config::AuthenticationConfig;
use crate::config::{NamedProfile, ProfileConfig, ProfileName};
use anyhow::{anyhow, Context};
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, Error};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsStr;
use std::fmt;
use std::fmt::{Debug, Formatter};
use std::io::Read;
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter, Serialize, Deserialize,
)]
pub enum GuestLanguage {
    Rust,
    TypeScript,
}

impl GuestLanguage {
    pub fn from_string(s: impl AsRef<str>) -> Option<GuestLanguage> {
        match s.as_ref().to_lowercase().as_str() {
            "rust" => Some(GuestLanguage::Rust),
            "ts" | "typescript" => Some(GuestLanguage::TypeScript),
            _ => None,
        }
    }

    pub fn from_id_string(s: impl AsRef<str>) -> Option<GuestLanguage> {
        match s.as_ref().to_lowercase().as_str() {
            "rust" => Some(GuestLanguage::Rust),
            "ts" => Some(GuestLanguage::TypeScript),
            _ => None,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            GuestLanguage::Rust => "rust",
            GuestLanguage::TypeScript => "ts",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GuestLanguage::Rust => "Rust",
            GuestLanguage::TypeScript => "TypeScript",
        }
    }
}

impl fmt::Display for GuestLanguage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for GuestLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        GuestLanguage::from_string(s).ok_or({
            let all = GuestLanguage::iter()
                .map(|x| format!("\"{x}\""))
                .collect::<Vec<String>>()
                .join(", ");
            format!("Unknown guest language: {s}. Expected one of {all}")
        })
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
    pub name: AppTemplateName,
    pub language: GuestLanguage,
    pub description: String,
}

impl TemplateDescription {
    pub fn from_template(template: &AppTemplate) -> Self {
        Self {
            name: template.name.clone(),
            language: template.language,
            description: template.description().to_string(),
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

pub struct NewInteractiveApp {
    pub app_name: ApplicationName,
    pub templated_component_names: Vec<(ComponentTemplateName, ComponentName)>,
}
