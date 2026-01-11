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

use crate::log::LogColorize;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

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
