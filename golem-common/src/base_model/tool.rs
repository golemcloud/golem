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
use crate::base_model::agent_config::CanonicalAgentConfigPath;
use crate::base_model::agent_secret::CanonicalAgentSecretPath;
use crate::base_model::component::{InitialAgentFile, InstalledPlugin};
use crate::base_model::diff::Hash;
use crate::base_model::json::NormalizedJsonValue;
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::model::agent::AgentTypeName;
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
#[cfg(feature = "full")]
use crate::model::entity::{
    EntityActivation, EntityActivationPolicy, ExecutableTarget, FilesystemCapability,
};
use crate::model::tool_release::{ToolReleaseId, ToolReleaseReference};
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "kind", content = "keys", rename_all = "camelCase")]
pub enum ConfigKeyScope {
    #[default]
    All,
    Keys(BTreeSet<CanonicalAgentConfigPath>),
}

impl ConfigKeyScope {
    pub fn contains(&self, key: &CanonicalAgentConfigPath) -> bool {
        match self {
            Self::All => true,
            Self::Keys(keys) => keys.contains(key),
        }
    }

    pub fn intersection(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::All, value) | (value, Self::All) => value.clone(),
            (Self::Keys(left), Self::Keys(right)) => {
                Self::Keys(left.intersection(right).cloned().collect())
            }
        }
    }
}

impl SecretKeyScope {
    pub fn contains(&self, key: &CanonicalAgentSecretPath) -> bool {
        match self {
            Self::All => true,
            Self::Keys(keys) => keys.contains(key),
        }
    }

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
    pub config_keys_readable: ConfigKeyScope,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default), oai(default))]
    pub filesystem_access: ToolFilesystemAccess,
    /// `None` means no middleware list was authored; `Some([])` explicitly clears the
    /// per-tool list when combined with `Replace`.
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default), oai(default))]
    pub middleware: Option<Vec<crate::base_model::tool_middleware::ToolMiddlewareInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "full", desert(default), oai(default))]
    pub middleware_merge_mode: Option<crate::base_model::tool_middleware::ToolMiddlewareMergeMode>,
}

impl Default for ToolBindingInput {
    fn default() -> Self {
        Self {
            version: None,
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            account: None,
            config_keys_readable: ConfigKeyScope::All,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
            middleware: None,
            middleware_merge_mode: None,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub enum ToolFilesystemAccess {
    #[default]
    Unset,
    Allowed,
    Denied,
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
    pub component_bindings: BTreeMap<ComponentName, ToolBindingInput>,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub agent_bindings: BTreeMap<AgentTypeName, ToolBindingInput>,
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
pub struct RemoteToolDeployment {
    pub name: ToolName,
    pub release: ToolReleaseReference,
    pub provision: ToolProvisionConfig,
    pub environment_binding: Option<ToolBindingInput>,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub component_bindings: BTreeMap<ComponentName, ToolBindingInput>,
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub agent_bindings: BTreeMap<AgentTypeName, ToolBindingInput>,
}

pub const TOOL_METADATA_WIT_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(try_from = "String", into = "String")]
pub struct HostToolId(String);

impl HostToolId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HostToolId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Host tool id", &value)?;
        Ok(Self(value))
    }
}

impl From<HostToolId> for String {
    fn from(value: HostToolId) -> Self {
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
pub enum ToolSource {
    Component {
        #[serde(rename = "componentId")]
        component_id: ComponentId,
        #[serde(rename = "componentRevision")]
        component_revision: ComponentRevision,
        #[serde(rename = "componentName")]
        component_name: ComponentName,
    },
    Host {
        #[serde(rename = "hostToolId")]
        host_tool_id: HostToolId,
        #[serde(rename = "implementationVersion")]
        implementation_version: String,
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
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub release_id: Option<ToolReleaseId>,
    pub definition: Tool,
    pub provision: ToolProvisionConfig,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    #[cfg_attr(feature = "full", oai(default))]
    pub component_bindings: BTreeMap<ComponentName, ToolBindingInput>,
    pub source: ToolSource,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub metadata_digest: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct CompiledToolBinding {
    pub deployment_revision: DeploymentRevision,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub release_id: Option<ToolReleaseId>,
    pub owner: ToolBindingOwner,
    pub tool_name: ToolName,
    pub version: String,
    pub metadata_version: String,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub metadata_digest: Hash,
    pub account_id: AccountId,
    pub account_email: AccountEmail,
    pub parameters: NormalizedJsonValue,
    pub config_keys_readable: ConfigKeyScope,
    pub secret_keys_readable: SecretKeyScope,
    pub secret_keys_revealable: SecretKeyScope,
    #[serde(default)]
    #[cfg_attr(feature = "full", desert(default))]
    pub filesystem_access: ToolFilesystemAccess,
    pub source: ToolSource,
}

/// The exact registration and binding accepted for a tool invocation.
///
/// Persisting this value with the invocation pins execution and replay to the accepted
/// deployment rather than resolving whichever deployment happens to be current later.
#[cfg(feature = "full")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, desert_rust::BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub struct ToolActivationSnapshot {
    pub registered_tool: RegisteredTool,
    pub binding: CompiledToolBinding,
    pub filesystem: FilesystemCapability,
    pub middleware_chain: Option<crate::model::tool_middleware::CompiledToolMiddlewareChain>,
}

#[cfg(feature = "full")]
#[derive(Clone, Debug, PartialEq)]
pub enum ToolDispatchTarget {
    Component(EntityActivation),
    Host {
        host_tool_id: HostToolId,
        implementation_version: String,
        deployment_revision: DeploymentRevision,
        provision: ToolProvisionConfig,
        binding: Box<CompiledToolBinding>,
        filesystem: FilesystemCapability,
    },
}

#[cfg(feature = "full")]
impl ToolActivationSnapshot {
    pub fn registered_tool(&self) -> &RegisteredTool {
        &self.registered_tool
    }

    pub fn binding(&self) -> &CompiledToolBinding {
        &self.binding
    }

    pub fn filesystem(&self) -> FilesystemCapability {
        self.filesystem
    }

    pub fn middleware_chain(
        &self,
    ) -> Option<&crate::model::tool_middleware::CompiledToolMiddlewareChain> {
        self.middleware_chain.as_ref()
    }

    pub fn effective_definition(&self) -> &Tool {
        self.middleware_chain
            .as_ref()
            .map(|chain| &chain.effective_definition)
            .unwrap_or(&self.registered_tool.definition)
    }

    pub fn into_dispatch_target(self) -> Result<ToolDispatchTarget, String> {
        match self.registered_tool.source {
            ToolSource::Component {
                component_id,
                component_revision,
                ..
            } => EntityActivation::new(
                ExecutableTarget::new(component_id, component_revision),
                self.registered_tool.deployment_revision,
                EntityActivationPolicy::Tool {
                    provision: self.registered_tool.provision,
                    binding: Box::new(self.binding),
                },
                self.filesystem,
            )
            .map(ToolDispatchTarget::Component),
            ToolSource::Host {
                host_tool_id,
                implementation_version,
            } => Ok(ToolDispatchTarget::Host {
                host_tool_id,
                implementation_version,
                deployment_revision: self.registered_tool.deployment_revision,
                provision: self.registered_tool.provision,
                binding: Box::new(self.binding),
                filesystem: self.filesystem,
            }),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "type", content = "value")]
pub enum SerializableToolError {
    InvalidToolName(String),
    InvalidCommandPath(Vec<String>),
    InvalidInput(String),
    ConstraintViolation(String),
    InvalidResult(String),
    CustomError(Box<SerializableCustomToolError>),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct SerializableCustomToolError {
    pub name: String,
    pub payload: crate::schema::TypedSchemaValue,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "type", content = "value")]
pub enum SerializableToolRpcError {
    ProtocolError(String),
    Denied(String),
    NotFound(String),
    RemoteInternalError(String),
    RemoteToolError(Box<SerializableToolError>),
    Cancelled,
    ResourceExhausted(String),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, golem_schema_derive::PoemSchema)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct SerializableToolInvocationResult {
    pub result: Option<Box<crate::schema::TypedSchemaValue>>,
}

/// Internal invocation value lowered onto the ordinary durable-stream transport.
#[derive(Clone, Debug, golem_schema_derive::FromSchema)]
pub struct ToolInvocationInput {
    pub arguments: crate::schema::TypedSchemaValue,
    pub stdin: Option<golem_schema::schema::SchemaValueStream>,
}

/// Internal result value. The stdout handle can be registered before the outcome is ready.
#[derive(Clone, Debug, golem_schema_derive::FromSchema)]
pub struct ToolInvocationOutput {
    pub outcome: Result<SerializableToolInvocationResult, SerializableToolRpcError>,
    pub stdout: Option<golem_schema::schema::SchemaValueStream>,
}

impl crate::schema::conversion::IntoSchema for ToolInvocationInput {
    fn type_id() -> crate::schema::TypeId {
        crate::schema::TypeId::new("golem.internal.ToolInvocationInput")
    }

    fn register_in(builder: &mut crate::schema::SchemaBuilder) -> crate::schema::SchemaType {
        use crate::schema::{NamedFieldType, SchemaType, TypedSchemaValue};
        SchemaType::record(vec![
            NamedFieldType {
                name: "arguments".into(),
                body: TypedSchemaValue::register_in(builder),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "stdin".into(),
                body: SchemaType::option(SchemaType::stream(Some(SchemaType::u8()))),
                metadata: Default::default(),
            },
        ])
    }

    fn to_value(&self) -> crate::schema::SchemaValue {
        crate::schema::SchemaValue::Record {
            fields: vec![self.arguments.to_value(), self.stdin.to_value()],
        }
    }
}

impl crate::schema::conversion::IntoSchema for ToolInvocationOutput {
    fn type_id() -> crate::schema::TypeId {
        crate::schema::TypeId::new("golem.internal.ToolInvocationOutput")
    }

    fn register_in(builder: &mut crate::schema::SchemaBuilder) -> crate::schema::SchemaType {
        use crate::schema::{NamedFieldType, SchemaType};
        SchemaType::record(vec![
            NamedFieldType { name: "outcome".into(), body: Result::<SerializableToolInvocationResult, SerializableToolRpcError>::register_in(builder), metadata: Default::default() },
            NamedFieldType { name: "stdout".into(), body: SchemaType::option(SchemaType::stream(Some(SchemaType::u8()))), metadata: Default::default() },
        ])
    }

    fn to_value(&self) -> crate::schema::SchemaValue {
        crate::schema::SchemaValue::Record {
            fields: vec![self.outcome.to_value(), self.stdout.to_value()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolBindingOwner {
    AgentType { agent_type_name: AgentTypeName },
    ComponentBaseline { component_id: ComponentId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct DeployedRegisteredTool {
    pub deployment_revision: DeploymentRevision,
    pub release_id: Option<ToolReleaseId>,
    pub definition: Tool,
    pub source: ToolSource,
    pub owner_account_id: AccountId,
    pub owner_account_email: AccountEmail,
    pub metadata_version: String,
    pub metadata_digest: Hash,
}

impl From<RegisteredTool> for DeployedRegisteredTool {
    fn from(value: RegisteredTool) -> Self {
        Self {
            deployment_revision: value.deployment_revision,
            release_id: value.release_id,
            definition: value.definition,
            source: value.source,
            owner_account_id: value.owner_account_id,
            owner_account_email: value.owner_account_email,
            metadata_version: value.metadata_version,
            metadata_digest: value.metadata_digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct ToolDeploymentState {
    pub deployment_revision: DeploymentRevision,
    pub registered_tools: BTreeMap<ToolName, RegisteredTool>,
    pub tool_bindings: BTreeMap<ToolBindingOwner, BTreeMap<ToolName, CompiledToolBinding>>,
    pub registered_tool_middlewares: BTreeMap<
        crate::model::tool_middleware::ToolMiddlewareName,
        crate::model::tool_middleware::RegisteredToolMiddleware,
    >,
    pub tool_middleware_chains: BTreeMap<
        ToolBindingOwner,
        BTreeMap<ToolName, crate::model::tool_middleware::CompiledToolMiddlewareChain>,
    >,
}

#[cfg(test)]
mod tests {
    use super::{ConfigKeyScope, SecretKeyScope, ToolName};
    use crate::model::agent_config::CanonicalAgentConfigPath;
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
            SecretKeyScope::Keys(BTreeSet::from([a.clone()]))
        );
        assert_eq!(left.intersection(&SecretKeyScope::All), left);
        assert!(left.contains(&a));
        assert!(!right.contains(&CanonicalAgentSecretPath(vec!["b".to_string()])));
        assert!(SecretKeyScope::All.contains(&CanonicalAgentSecretPath(vec!["c".to_string()])));
    }

    #[test]
    fn config_key_scope_intersection_allows_only_shared_keys() {
        let a = CanonicalAgentConfigPath(vec!["a".to_string()]);
        let b = CanonicalAgentConfigPath(vec!["b".to_string()]);
        let environment = ConfigKeyScope::Keys(BTreeSet::from([a.clone(), b]));
        let agent = ConfigKeyScope::Keys(BTreeSet::from([a.clone()]));

        let effective = environment.intersection(&agent);
        assert!(effective.contains(&a));
        assert!(!effective.contains(&CanonicalAgentConfigPath(vec!["b".to_string()])));
        assert_eq!(ConfigKeyScope::All.intersection(&agent), agent);
    }
}
