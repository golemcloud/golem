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
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::model::json::NormalizedJsonValue;
use crate::model::tool_middleware_release::{
    ToolMiddlewareReleaseId, ToolMiddlewareReleaseReference,
};
use crate::schema::TypedSchemaValue;
use crate::schema::tool::compatibility::CompiledToolCompatibility;
use crate::schema::tool::{Tool, ToolMiddleware};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::NewType))]
#[cfg_attr(
    feature = "full",
    oai(from_json = false, from_parameter = false, from_multipart = false)
)]
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

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromJSON for ToolMiddlewareName {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let raw = <String as poem_openapi::types::ParseFromJSON>::parse_from_json(value)
            .map_err(poem_openapi::types::ParseError::propagate)?;
        Self::try_from(raw).map_err(poem_openapi::types::ParseError::custom)
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromParameter for ToolMiddlewareName {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        Self::try_from(value).map_err(poem_openapi::types::ParseError::custom)
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromMultipartField for ToolMiddlewareName {
    async fn parse_from_multipart(
        field: Option<poem::web::Field>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let raw =
            <String as poem_openapi::types::ParseFromMultipartField>::parse_from_multipart(field)
                .await
                .map_err(poem_openapi::types::ParseError::propagate)?;
        Self::try_from(raw).map_err(poem_openapi::types::ParseError::custom)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for ToolMiddlewareName {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&self.0, context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for ToolMiddlewareName {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let value = <String as desert_rust::BinaryDeserializer>::deserialize(context)?;
        Self::try_from(value).map_err(|e| {
            desert_rust::Error::DeserializationFailure(format!("Invalid ToolMiddlewareName: {e}"))
        })
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
    pub secret_keys_readable: Option<SecretKeyScope>,
    pub secret_keys_revealable: Option<SecretKeyScope>,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default), oai(default))]
    pub filesystem_access: ToolFilesystemAccess,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
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

/// Deployment-owned inputs for compiling middleware against dynamic tool metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ToolMiddlewareConfiguration {
    pub universal: Vec<ToolMiddlewareInstallation>,
    pub compatibility_mode: crate::schema::tool::compatibility::ToolCompatibilityMode,
    pub environment_bindings:
        std::collections::BTreeMap<ToolName, crate::model::tool::ToolBindingInput>,
    pub agent_bindings: std::collections::BTreeMap<
        crate::model::agent::AgentTypeName,
        std::collections::BTreeMap<ToolName, crate::model::tool::ToolBindingInput>,
    >,
}

/// One pinned middleware invocation in a compiled tool chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct CompiledToolMiddlewareOccurrence {
    pub middleware: RegisteredToolMiddleware,
    pub parameters: TypedSchemaValue,
    pub provision: ToolProvisionConfig,
    pub config_keys_readable: crate::model::tool::ConfigKeyScope,
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
    pub owner: crate::model::tool::ToolBindingOwner,
    pub tool_name: ToolName,
    pub effective_definition: Tool,
    pub occurrences: Vec<CompiledToolMiddlewareOccurrence>,
}

#[cfg(all(test, feature = "full"))]
mod name_codec_tests {
    use super::{ToolMiddlewareInstallation, ToolMiddlewareName};
    use crate::base_model::tool::name_codec_tests::{
        assert_desert_bytes_equal_string_bytes, assert_poem_and_desert_match_serde,
        assert_schema_and_json_are_a_plain_string,
    };
    use poem_openapi::types::ParseFromJSON;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn tool_middleware_name_poem_and_desert_accept_exactly_what_serde_accepts() {
        assert_poem_and_desert_match_serde::<ToolMiddlewareName>();
        assert!(ToolMiddlewareName::try_from("Audit_Log").is_err());
    }

    #[test]
    fn tool_middleware_name_poem_schema_and_json_are_a_plain_string() {
        assert_schema_and_json_are_a_plain_string::<ToolMiddlewareName>();
    }

    #[test]
    fn tool_middleware_name_desert_bytes_equal_string_bytes() {
        assert_desert_bytes_equal_string_bytes::<ToolMiddlewareName>();
    }

    #[test]
    fn installation_with_an_invalid_name_is_refused_by_poem() {
        assert!(
            ToolMiddlewareInstallation::parse_from_json(Some(
                json!({"name": "Audit_Log", "parameters": {}})
            ))
            .is_err()
        );
        assert!(
            ToolMiddlewareInstallation::parse_from_json(Some(
                json!({"name": "audit-log", "parameters": {}})
            ))
            .is_ok()
        );
    }
}
