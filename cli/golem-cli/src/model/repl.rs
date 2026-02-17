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

use crate::model::app::CustomBridgeSdkTarget;
use crate::model::environment::ResolvedEnvironmentIdentity;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::model::component::ComponentName;
use golem_templates::model::GuestLanguage;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter)]
pub enum ReplLanguage {
    Rib,
    Rust,
    TypeScript,
}

impl ReplLanguage {
    pub fn from_string(s: impl AsRef<str>) -> Option<ReplLanguage> {
        match s.as_ref().to_lowercase().as_str() {
            "rib" => Some(ReplLanguage::Rib),
            "rust" => Some(ReplLanguage::Rust),
            "ts" | "typescript" => Some(ReplLanguage::TypeScript),
            _ => None,
        }
    }

    pub fn is_rib(&self) -> bool {
        matches!(self, ReplLanguage::Rib)
    }

    pub fn to_guest_language(&self) -> Option<GuestLanguage> {
        match self {
            ReplLanguage::Rib => None,
            ReplLanguage::Rust => Some(GuestLanguage::Rust),
            ReplLanguage::TypeScript => Some(GuestLanguage::TypeScript),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            ReplLanguage::Rib => "rib",
            ReplLanguage::Rust => "rust",
            ReplLanguage::TypeScript => "ts",
        }
    }
}

impl Display for ReplLanguage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplLanguage::Rib => write!(f, "Rib"),
            ReplLanguage::Rust => write!(f, "Rust"),
            ReplLanguage::TypeScript => write!(f, "TypeScript"),
        }
    }
}

impl FromStr for ReplLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ReplLanguage::from_string(s).ok_or({
            let all = ReplLanguage::iter()
                .map(|x| format!("\"{x}\""))
                .collect::<Vec<String>>()
                .join(", ");
            format!("Unknown guest language: {s}. Expected one of {all}")
        })
    }
}

impl From<GuestLanguage> for ReplLanguage {
    fn from(guest_language: GuestLanguage) -> Self {
        match guest_language {
            GuestLanguage::Rust => ReplLanguage::Rust,
            GuestLanguage::TypeScript => ReplLanguage::TypeScript,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BridgeReplArgs {
    pub environment: ResolvedEnvironmentIdentity,
    pub component_names: Vec<ComponentName>,
    pub script: Option<ReplScriptSource>,
    pub stream_logs: bool,
    pub disable_auto_imports: bool,
    pub app_main_dir: PathBuf,
    pub repl_root_dir: PathBuf,
    pub repl_root_bridge_sdk_dir: PathBuf,
    pub repl_metadata_json_path: PathBuf,
    pub repl_cli_commands_metadata_json_path: PathBuf,
    pub repl_bridge_sdk_target: CustomBridgeSdkTarget,
    pub repl_history_file_path: PathBuf,
}

#[derive(Clone, Debug)]
pub enum ReplScriptSource {
    Inline(String),
    FromFile(PathBuf),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplMetadata {
    pub agents: HashMap<AgentTypeName, ReplAgentMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplAgentMetadata {
    pub client_dir: PathBuf,
}
