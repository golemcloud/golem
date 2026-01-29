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

use crate::model::environment::ResolvedEnvironmentIdentity;
use golem_common::base_model::agent::AgentTypeName;
use golem_templates::model::GuestLanguage;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Clone, Debug, Copy, PartialEq, Eq, EnumIter)]
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

#[derive(Clone, Debug)]
pub struct BridgeReplArgs {
    pub environment: ResolvedEnvironmentIdentity,
    pub script: Option<String>,
    pub stream_logs: bool,
    pub repl_root_dir: PathBuf,
    pub repl_root_bridge_sdk_dir: PathBuf,
    pub agent_type_names: Vec<AgentTypeName>,
}
