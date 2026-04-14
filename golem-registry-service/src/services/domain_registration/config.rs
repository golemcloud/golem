// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_common::SafeDisplay;
use golem_common::model::Empty;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::fmt::Write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainRegistrationConfig {
    pub available_domains: AvailableDomainsConfig,
}

impl SafeDisplay for DomainRegistrationConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "available domains:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.available_domains.to_safe_string_indented()
        );
        result
    }
}

impl Default for DomainRegistrationConfig {
    fn default() -> Self {
        Self {
            available_domains: AvailableDomainsConfig::Unrestricted(Empty {}),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum AvailableDomainsConfig {
    Restricted(RestrictedAvailableDomainsConfig),
    Unrestricted(Empty),
}

impl SafeDisplay for AvailableDomainsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            Self::Restricted(inner) => {
                let _ = writeln!(&mut result, "restricted:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            Self::Unrestricted(_) => {
                let _ = writeln!(&mut result, "unrestricted");
            }
        }
        result
    }
}

impl Default for AvailableDomainsConfig {
    fn default() -> Self {
        Self::Unrestricted(Empty {})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestrictedAvailableDomainsConfig {
    pub golem_apps_domain: String,
    pub allow_arbitary_subdomains: bool,
}

impl SafeDisplay for RestrictedAvailableDomainsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "golem apps domain: {}", self.golem_apps_domain);
        let _ = writeln!(
            &mut result,
            "allow arbitrary subdomains: {}",
            self.allow_arbitary_subdomains
        );
        result
    }
}

impl Default for RestrictedAvailableDomainsConfig {
    fn default() -> Self {
        Self {
            golem_apps_domain: "apps.golem.cloud".to_string(),
            allow_arbitary_subdomains: false,
        }
    }
}
