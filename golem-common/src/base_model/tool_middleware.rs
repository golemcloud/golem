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
use crate::base_model::diff::Hash;
use crate::base_model::tool::ToolProvisionConfig;
use crate::base_model::tool::{SecretKeyScope, ToolFilesystemAccess, ToolName};
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::model::agent::AgentTypeName;
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::model::json::NormalizedJsonValue;
use crate::model::tool_middleware_release::{
    ToolMiddlewareReleaseId, ToolMiddlewareReleaseReference,
};
use crate::schema::tool::compatibility::CompiledToolCompatibility;
use crate::schema::tool::{Tool, ToolMiddleware};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(try_from = "String", into = "String")]
pub struct ToolMiddlewareName(String);

impl ToolMiddlewareName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Display for ToolMiddlewareName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<&str> for ToolMiddlewareName {
    type Error = String;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Tool middleware name", value)?;
        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for ToolMiddlewareName {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl FromStr for ToolMiddlewareName {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl From<ToolMiddlewareName> for String {
    fn from(value: ToolMiddlewareName) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolMiddlewareSource {
    Component {
        #[serde(rename = "componentId")]
        component_id: ComponentId,
        #[serde(rename = "componentRevision")]
        component_revision: ComponentRevision,
        #[serde(rename = "componentName")]
        component_name: ComponentName,
    },
}

pub const TOOL_MIDDLEWARE_METADATA_WIT_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ToolMiddlewareInstallation {
    pub name: ToolMiddlewareName,
    pub version: Option<String>,
    pub parameters: NormalizedJsonValue,
    pub account: Option<AccountEmail>,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default), oai(default))]
    pub filesystem_access: ToolFilesystemAccess,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub enum ToolMiddlewareMergeMode {
    #[default]
    Prepend,
    Append,
    Replace,
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
pub struct RegisteredToolMiddleware {
    pub deployment_revision: DeploymentRevision,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub release_id: Option<ToolMiddlewareReleaseId>,
    pub definition: ToolMiddleware,
    pub provision: ToolProvisionConfig,
    pub source: ToolMiddlewareSource,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub metadata_digest: Hash,
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
pub struct ToolMiddlewareDeploymentMetadata {
    pub definition: ToolMiddleware,
    pub provision: ToolProvisionConfig,
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
pub struct RemoteToolMiddlewareDeployment {
    pub name: ToolMiddlewareName,
    pub release: ToolMiddlewareReleaseReference,
    pub provision: ToolProvisionConfig,
}

/// One pinned middleware invocation in a compiled tool chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct CompiledToolMiddlewareOccurrence {
    pub middleware: RegisteredToolMiddleware,
    pub parameters: NormalizedJsonValue,
    pub provision: ToolProvisionConfig,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    pub filesystem_access: ToolFilesystemAccess,
    pub expected_definition: Option<Tool>,
    pub presented_definition: Option<Tool>,
    pub next_effective_definition: Tool,
    pub compatibility: Option<CompiledToolCompatibility>,
}

/// The immutable middleware chain selected for one agent/tool activation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct CompiledToolMiddlewareChain {
    pub deployment_revision: DeploymentRevision,
    pub agent_type_name: AgentTypeName,
    pub tool_name: ToolName,
    pub effective_definition: Tool,
    pub occurrences: Vec<CompiledToolMiddlewareOccurrence>,
}
