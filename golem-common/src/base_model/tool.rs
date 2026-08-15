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

use crate::base_model::account::{AccountEmail, AccountId};
use crate::base_model::agent_secret::CanonicalAgentSecretPath;
use crate::base_model::component::{InitialAgentFile, InstalledPlugin};
use crate::base_model::json::NormalizedJsonValue;
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::model::agent::AgentTypeName;
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::schema::tool::Tool;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(try_from = "String", into = "String")]
pub struct ToolName(String);

impl ToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Display for ToolName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<&str> for ToolName {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Tool name", value)?;
        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for ToolName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for ToolName {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl From<ToolName> for String {
    fn from(value: ToolName) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", content = "keys", rename_all = "camelCase")]
pub enum SecretKeyScope {
    #[default]
    All,
    Keys(BTreeSet<CanonicalAgentSecretPath>),
}

impl SecretKeyScope {
    pub fn intersection(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::All, value) | (value, Self::All) => value.clone(),
            (Self::Keys(left), Self::Keys(right)) => {
                Self::Keys(left.intersection(right).cloned().collect())
            }
        }
    }

    pub fn is_subset_of(&self, other: &Self) -> bool {
        match (self, other) {
            (_, Self::All) => true,
            (Self::All, Self::Keys(_)) => false,
            (Self::Keys(left), Self::Keys(right)) => left.is_subset(right),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ToolBindingInput {
    pub version: Option<String>,
    pub parameters: NormalizedJsonValue,
    pub account: Option<AccountEmail>,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
}

impl Default for ToolBindingInput {
    fn default() -> Self {
        Self {
            version: None,
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            account: None,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ToolProvisionConfig {
    pub config: NormalizedJsonValue,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub plugins: Vec<InstalledPlugin>,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub files: Vec<InitialAgentFile>,
}

impl Default for ToolProvisionConfig {
    fn default() -> Self {
        Self {
            config: NormalizedJsonValue::new(serde_json::json!({})),
            env: BTreeMap::new(),
            plugins: Vec::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct ToolDeploymentMetadata {
    pub definition: Tool,
    pub provision: ToolProvisionConfig,
    pub environment_binding: Option<ToolBindingInput>,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub agent_bindings: BTreeMap<AgentTypeName, ToolBindingInput>,
}

pub const TOOL_METADATA_WIT_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSource {
    Component {
        #[serde(rename = "componentId")]
        component_id: ComponentId,
        #[serde(rename = "componentRevision")]
        component_revision: ComponentRevision,
        #[serde(rename = "componentName")]
        component_name: ComponentName,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct RegisteredTool {
    pub deployment_revision: DeploymentRevision,
    pub definition: Tool,
    pub provision: ToolProvisionConfig,
    pub source: ToolSource,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CompiledToolBinding {
    pub deployment_revision: DeploymentRevision,
    pub agent_type_name: AgentTypeName,
    pub tool_name: ToolName,
    pub version: String,
    pub metadata_version: String,
    pub account_id: AccountId,
    pub account_email: AccountEmail,
    pub parameters: NormalizedJsonValue,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    pub source: ToolSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct DeployedRegisteredTool {
    pub deployment_revision: DeploymentRevision,
    pub definition: Tool,
    pub source: ToolSource,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
}

impl From<RegisteredTool> for DeployedRegisteredTool {
    fn from(value: RegisteredTool) -> Self {
        Self {
            deployment_revision: value.deployment_revision,
            definition: value.definition,
            source: value.source,
            owner_account_id: value.owner_account_id,
            owner_account_email: value.owner_account_email,
            metadata_version: value.metadata_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct ToolDeploymentState {
    pub deployment_revision: DeploymentRevision,
    pub registered_tools: BTreeMap<ToolName, RegisteredTool>,
    pub agent_tool_bindings: BTreeMap<AgentTypeName, BTreeMap<ToolName, CompiledToolBinding>>,
}

#[cfg(test)]
mod tests {
    use super::{SecretKeyScope, ToolName};
    use crate::model::agent_secret::CanonicalAgentSecretPath;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn tool_name_uses_lower_kebab_case_identifier_grammar() {
        for valid in ["a", "grep", "git-client", "tool2", "a-2b"] {
            assert_eq!(ToolName::try_from(valid).unwrap().as_str(), valid);
        }

        for invalid in ["", "Grep", "git_client", "2tool", "tool-", "tool--x"] {
            assert!(
                ToolName::try_from(invalid).is_err(),
                "expected '{invalid}' to be rejected"
            );
        }
    }

    #[test]
    fn secret_key_scope_intersection_never_widens() {
        let a = CanonicalAgentSecretPath(vec!["a".to_string()]);
        let b = CanonicalAgentSecretPath(vec!["b".to_string()]);
        let left = SecretKeyScope::Keys(BTreeSet::from([a.clone(), b]));
        let right = SecretKeyScope::Keys(BTreeSet::from([a.clone()]));

        assert_eq!(
            left.intersection(&right),
            SecretKeyScope::Keys(BTreeSet::from([a]))
        );
        assert_eq!(left.intersection(&SecretKeyScope::All), left);
    }
}
