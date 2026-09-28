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

use crate::base_model::agent::Principal;
use crate::model::component::{ComponentId, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::model::oplog::OplogIndex;
use crate::model::oplog::SpanStarted;
use crate::model::tool::{
    CompiledToolBinding, HostToolId, SecretKeyScope, ToolFilesystemAccess, ToolName,
    ToolProvisionConfig,
};
use crate::model::{IdempotencyKey, OwnedAgentId};
use crate::schema::TypedSchemaValue;
use crate::schema::tool::Tool;
use crate::schema::tool::compatibility::CompiledToolCompatibility;
use desert_rust::BinaryCodec;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

#[cfg(test)]
mod tests;

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, BinaryCodec,
)]
#[serde(tag = "kind", content = "name", rename_all = "camelCase")]
pub enum AgentEntity {
    Tool(ToolName),
    ToolMiddleware(ToolMiddlewareName),
}

impl AgentEntity {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::ToolMiddleware(_) => "tool_middleware",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Tool(name) => name.as_str(),
            Self::ToolMiddleware(name) => name.as_str(),
        }
    }
}

impl Display for AgentEntity {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(name) => write!(f, "tool:{name}"),
            Self::ToolMiddleware(name) => write!(f, "tool-middleware:{name}"),
        }
    }
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, BinaryCodec,
)]
#[desert(transparent)]
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

impl TryFrom<&str> for ToolMiddlewareName {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        crate::base_model::validate_lower_kebab_case_identifier("Tool middleware name", value)?;
        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for ToolMiddlewareName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<ToolMiddlewareName> for String {
    fn from(value: ToolMiddlewareName) -> Self {
        value.0
    }
}

impl Display for ToolMiddlewareName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "entity", rename_all = "camelCase")]
pub enum OwnerRuntime {
    Agent,
    Entity(AgentEntity),
}

impl OwnerRuntime {
    pub fn entity(&self) -> Option<&AgentEntity> {
        match self {
            Self::Agent => None,
            Self::Entity(entity) => Some(entity),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedAgentEntityId {
    pub owner: OwnedAgentId,
    pub entity: AgentEntity,
}

impl OwnedAgentEntityId {
    pub fn owner_id(&self) -> &OwnedAgentId {
        &self.owner
    }

    pub fn into_owner_id(self) -> OwnedAgentId {
        self.owner
    }
}

impl Display for OwnedAgentEntityId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.entity)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityInvocationId {
    entity_id: OwnedAgentEntityId,
    start_index: OplogIndex,
}

impl EntityInvocationId {
    pub fn new(entity_id: OwnedAgentEntityId, start_index: OplogIndex) -> Result<Self, String> {
        if start_index == OplogIndex::NONE {
            return Err("Entity invocation Start index cannot be zero".to_string());
        }
        Ok(Self {
            entity_id,
            start_index,
        })
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        self.entity_id.owner_id()
    }

    pub fn entity(&self) -> &AgentEntity {
        &self.entity_id.entity
    }

    pub fn entity_id(&self) -> &OwnedAgentEntityId {
        &self.entity_id
    }

    pub fn start_index(&self) -> OplogIndex {
        self.start_index
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityInvocationIdWire {
    entity_id: OwnedAgentEntityId,
    start_index: OplogIndex,
}

impl<'de> Deserialize<'de> for EntityInvocationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = EntityInvocationIdWire::deserialize(deserializer)?;
        Self::new(wire.entity_id, wire.start_index).map_err(D::Error::custom)
    }
}

impl Display for EntityInvocationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.entity_id, self.start_index)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub struct ExecutableTarget {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
}

/// Exact executable leaf pinned into an accepted entity invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EntityActivationSource {
    Component {
        executable: ExecutableTarget,
    },
    Host {
        host_tool_id: HostToolId,
        implementation_version: String,
    },
}

impl ExecutableTarget {
    pub fn new(component_id: ComponentId, component_revision: ComponentRevision) -> Self {
        Self {
            component_id,
            component_revision,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[serde(rename_all = "camelCase")]
pub enum FilesystemCapability {
    Capable,
    Incapable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub struct McpImportActivation {
    pub source: crate::model::mcp_import::McpImportSource,
    pub protocol_version: String,
    pub projected_tool: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EntityActivationPolicy {
    Tool {
        provision: ToolProvisionConfig,
        binding: Box<CompiledToolBinding>,
        mcp_import: Option<Box<McpImportActivation>>,
    },
    ToolMiddleware {
        middleware_name: ToolMiddlewareName,
        provision: ToolProvisionConfig,
        config_keys_readable: crate::model::tool::ConfigKeyScope,
        secret_keys_readable: SecretKeyScope,
        secret_keys_revealable: SecretKeyScope,
        filesystem_access: ToolFilesystemAccess,
    },
}

impl EntityActivationPolicy {
    pub fn entity(&self) -> AgentEntity {
        match self {
            Self::Tool { binding, .. } => AgentEntity::Tool(binding.tool_name.clone()),
            Self::ToolMiddleware {
                middleware_name, ..
            } => AgentEntity::ToolMiddleware(middleware_name.clone()),
        }
    }

    pub fn provision(&self) -> &ToolProvisionConfig {
        match self {
            Self::Tool { provision, .. } | Self::ToolMiddleware { provision, .. } => provision,
        }
    }

    pub fn secret_keys_readable(&self) -> &SecretKeyScope {
        match self {
            Self::Tool { binding, .. } => &binding.secret_keys_readable,
            Self::ToolMiddleware {
                secret_keys_readable,
                ..
            } => secret_keys_readable,
        }
    }

    pub fn config_keys_readable(&self) -> &crate::model::tool::ConfigKeyScope {
        match self {
            Self::Tool { binding, .. } => &binding.config_keys_readable,
            Self::ToolMiddleware {
                config_keys_readable,
                ..
            } => config_keys_readable,
        }
    }

    pub fn secret_keys_revealable(&self) -> &SecretKeyScope {
        match self {
            Self::Tool { binding, .. } => &binding.secret_keys_revealable,
            Self::ToolMiddleware {
                secret_keys_revealable,
                ..
            } => secret_keys_revealable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, BinaryCodec)]
#[desert(transparent)]
#[serde(transparent)]
pub struct EntityActivationFingerprint([u8; 32]);

impl EntityActivationFingerprint {
    pub fn from_bytes(value: [u8; 32]) -> Self {
        Self(value)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Display for EntityActivationFingerprint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub struct EntityActivation {
    source: EntityActivationSource,
    deployment_revision: DeploymentRevision,
    policy: EntityActivationPolicy,
    filesystem: FilesystemCapability,
    fingerprint: EntityActivationFingerprint,
}

#[derive(BinaryCodec)]
#[desert(evolution())]
struct EntityActivationFingerprintInput {
    source: EntityActivationSource,
    deployment_revision: DeploymentRevision,
    policy: EntityActivationPolicy,
    filesystem: FilesystemCapability,
}

impl EntityActivation {
    pub fn new(
        executable: ExecutableTarget,
        deployment_revision: DeploymentRevision,
        policy: EntityActivationPolicy,
        filesystem: FilesystemCapability,
    ) -> Result<Self, String> {
        Self::new_with_source(
            EntityActivationSource::Component { executable },
            deployment_revision,
            policy,
            filesystem,
        )
    }

    pub fn new_host(
        host_tool_id: HostToolId,
        implementation_version: String,
        deployment_revision: DeploymentRevision,
        policy: EntityActivationPolicy,
        filesystem: FilesystemCapability,
    ) -> Result<Self, String> {
        Self::new_with_source(
            EntityActivationSource::Host {
                host_tool_id,
                implementation_version,
            },
            deployment_revision,
            policy,
            filesystem,
        )
    }

    fn new_with_source(
        source: EntityActivationSource,
        deployment_revision: DeploymentRevision,
        policy: EntityActivationPolicy,
        filesystem: FilesystemCapability,
    ) -> Result<Self, String> {
        Self::validate(&source, deployment_revision, &policy, filesystem)?;
        let fingerprint_input = EntityActivationFingerprintInput {
            source: source.clone(),
            deployment_revision,
            policy: policy.clone(),
            filesystem,
        };
        let bytes = desert_rust::serialize_to_byte_vec(&fingerprint_input)
            .map_err(|error| format!("Failed to fingerprint entity activation: {error}"))?;
        let fingerprint = EntityActivationFingerprint::from_bytes(*blake3::hash(&bytes).as_bytes());

        Ok(Self {
            source,
            deployment_revision,
            policy,
            filesystem,
            fingerprint,
        })
    }

    fn validate(
        source: &EntityActivationSource,
        deployment_revision: DeploymentRevision,
        policy: &EntityActivationPolicy,
        filesystem: FilesystemCapability,
    ) -> Result<(), String> {
        if let EntityActivationSource::Host {
            host_tool_id,
            implementation_version,
        } = source
        {
            if host_tool_id.as_str().is_empty() {
                return Err("Entity host source host tool id cannot be empty".to_string());
            }
            if implementation_version.trim().is_empty() {
                return Err("Entity host source implementation version cannot be empty".to_string());
            }
        }

        match policy {
            EntityActivationPolicy::Tool {
                provision,
                binding,
                mcp_import,
            } => {
                if binding.deployment_revision != deployment_revision {
                    return Err(
                        "Entity activation and tool binding deployment revisions differ"
                            .to_string(),
                    );
                }
                match (&binding.source, source) {
                    (
                        crate::model::tool::ToolSource::Component {
                            component_id,
                            component_revision,
                            ..
                        },
                        EntityActivationSource::Component { executable },
                    ) => {
                        if *component_id != executable.component_id
                            || *component_revision != executable.component_revision
                        {
                            return Err("Entity executable does not match the tool binding source"
                                .to_string());
                        }
                    }
                    (
                        crate::model::tool::ToolSource::Host {
                            host_tool_id,
                            implementation_version,
                        },
                        EntityActivationSource::Host {
                            host_tool_id: actual_id,
                            implementation_version: actual_version,
                        },
                    ) => {
                        if host_tool_id != actual_id || implementation_version != actual_version {
                            return Err(
                                "Entity host source does not match the tool binding source"
                                    .to_string(),
                            );
                        }
                    }
                    _ => {
                        return Err(
                            "Entity activation kind does not match the tool binding source"
                                .to_string(),
                        );
                    }
                }
                let is_mcp_bridge =
                    binding.source == crate::model::mcp_import::mcp_import_bridge_source();
                if is_mcp_bridge != mcp_import.is_some() {
                    return Err(
                        "MCP bridge activation requires its dynamic projection exclusively".into(),
                    );
                }
                if let Some(import) = mcp_import
                    && (import.source.deployment_revision != deployment_revision
                        || import.source.upstream_tool_name.is_empty()
                        || import.protocol_version.is_empty()
                        || import.projected_tool.is_empty())
                {
                    return Err("Invalid MCP import activation snapshot".into());
                }
                if !binding
                    .secret_keys_revealable
                    .is_subset_of(&binding.secret_keys_readable)
                {
                    return Err(
                        "Entity binding revealable secrets exceed readable secrets".to_string()
                    );
                }
                Self::validate_filesystem(
                    binding.filesystem_access,
                    provision,
                    filesystem,
                    "compiled tool binding",
                )?;
            }
            EntityActivationPolicy::ToolMiddleware {
                provision,
                secret_keys_readable,
                secret_keys_revealable,
                filesystem_access,
                ..
            } => {
                if matches!(source, EntityActivationSource::Host { .. }) {
                    return Err(
                        "Host entity activation is not supported for tool middleware".to_string(),
                    );
                }
                if !secret_keys_revealable.is_subset_of(secret_keys_readable) {
                    return Err(
                        "Entity middleware revealable secrets exceed readable secrets".to_string(),
                    );
                }
                Self::validate_filesystem(
                    *filesystem_access,
                    provision,
                    filesystem,
                    "compiled tool middleware policy",
                )?;
            }
        }
        Ok(())
    }

    fn validate_filesystem(
        filesystem_access: ToolFilesystemAccess,
        provision: &ToolProvisionConfig,
        filesystem: FilesystemCapability,
        policy_name: &str,
    ) -> Result<(), String> {
        let expected_filesystem = match (filesystem_access, provision.files.is_empty()) {
            (ToolFilesystemAccess::Allowed, _) | (ToolFilesystemAccess::Unset, false) => {
                FilesystemCapability::Capable
            }
            (ToolFilesystemAccess::Denied, false) => {
                return Err(
                    "Filesystem-denied entity activation cannot provision files".to_string()
                );
            }
            (ToolFilesystemAccess::Denied | ToolFilesystemAccess::Unset, true) => {
                FilesystemCapability::Incapable
            }
        };
        if filesystem != expected_filesystem {
            return Err(format!(
                "Entity filesystem capability {filesystem:?} does not match the {policy_name}"
            ));
        }
        Ok(())
    }

    pub fn source(&self) -> &EntityActivationSource {
        &self.source
    }

    pub fn executable_opt(&self) -> Option<&ExecutableTarget> {
        match &self.source {
            EntityActivationSource::Component { executable } => Some(executable),
            EntityActivationSource::Host { .. } => None,
        }
    }

    pub fn deployment_revision(&self) -> DeploymentRevision {
        self.deployment_revision
    }

    pub fn policy(&self) -> &EntityActivationPolicy {
        &self.policy
    }

    pub fn entity(&self) -> AgentEntity {
        self.policy.entity()
    }

    pub fn filesystem(&self) -> FilesystemCapability {
        self.filesystem
    }

    pub fn fingerprint(&self) -> EntityActivationFingerprint {
        self.fingerprint
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityActivationWire {
    source: EntityActivationSource,
    deployment_revision: DeploymentRevision,
    policy: EntityActivationPolicy,
    filesystem: FilesystemCapability,
    fingerprint: EntityActivationFingerprint,
}

impl<'de> Deserialize<'de> for EntityActivation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = EntityActivationWire::deserialize(deserializer)?;
        let activation = Self::new_with_source(
            wire.source,
            wire.deployment_revision,
            wire.policy,
            wire.filesystem,
        )
        .map_err(D::Error::custom)?;
        if activation.fingerprint != wire.fingerprint {
            return Err(D::Error::custom(
                "EntityActivation fingerprint does not match its contents",
            ));
        }
        Ok(activation)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[serde(rename_all = "camelCase")]
pub enum InvocationExecutionMode {
    Live,
    ReplayingCompleted,
    ReplayingIncomplete,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum EntityCallMode {
    Synchronous,
    Asynchronous,
    FireAndForget,
}

/// Semantic operation data pinned into an entity invocation `Start`. Resource-table keys and live
/// attachment state are intentionally excluded: only facts needed to reconstruct dispatch belong
/// in the owner oplog.
#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum EntityInvocationDescriptor {
    Tool(ToolInvocationDescriptor),
}

/// Immutable, outermost-to-innermost entity activations selected for one tool call. The complete
/// plan is stored only on the root entity `Start`; child invocations identify a layer in that
/// record with [`EntityInvocationPlanReference::Descendant`].
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct EntityInvocationPlan {
    layers: Vec<EntityInvocationPlanLayer>,
}

impl EntityInvocationPlan {
    pub fn new(layers: Vec<EntityInvocationPlanLayer>) -> Result<Self, String> {
        Self::validate_layers(&layers)?;
        Ok(Self { layers })
    }

    fn validate_layers(layers: &[EntityInvocationPlanLayer]) -> Result<(), String> {
        if layers.is_empty() {
            return Err("Entity invocation plan cannot be empty".to_string());
        }
        if !matches!(layers.last(), Some(EntityInvocationPlanLayer::Tool { .. })) {
            return Err("Entity invocation plan must end in a tool activation".to_string());
        }
        if layers[..layers.len() - 1]
            .iter()
            .any(|layer| !matches!(layer, EntityInvocationPlanLayer::Middleware { .. }))
        {
            return Err("Only middleware activations may precede the tool leaf".to_string());
        }
        for layer in layers {
            let valid = matches!(
                (layer, layer.activation().policy()),
                (
                    EntityInvocationPlanLayer::Middleware { .. },
                    EntityActivationPolicy::ToolMiddleware { .. }
                ) | (
                    EntityInvocationPlanLayer::Tool { .. },
                    EntityActivationPolicy::Tool { .. }
                )
            );
            if !valid {
                return Err(
                    "Entity invocation plan layer does not match its activation policy".to_string(),
                );
            }
        }
        let leaf_binding = match layers.last().unwrap().activation().policy() {
            EntityActivationPolicy::Tool { binding, .. } => binding,
            EntityActivationPolicy::ToolMiddleware { .. } => unreachable!(),
        };
        for layer in &layers[..layers.len() - 1] {
            let policy = layer.activation().policy();
            if !policy
                .secret_keys_readable()
                .is_subset_of(&leaf_binding.secret_keys_readable)
                || !policy
                    .secret_keys_revealable()
                    .is_subset_of(&leaf_binding.secret_keys_revealable)
                || !policy
                    .secret_keys_revealable()
                    .is_subset_of(policy.secret_keys_readable())
            {
                return Err(
                    "Entity middleware secret policy exceeds the recorded leaf binding".to_string(),
                );
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        Self::validate_layers(&self.layers)
    }

    pub fn layer(&self, position: u32) -> Result<&EntityInvocationPlanLayer, String> {
        self.layers
            .get(position as usize)
            .ok_or_else(|| format!("Entity invocation plan position {position} is out of bounds"))
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[allow(
    clippy::large_enum_variant,
    reason = "A plan contains middleware layers followed by exactly one tool; boxing would add indirection to every middleware to save space for only the terminal tool"
)]
pub enum EntityInvocationPlanLayer {
    Middleware {
        activation: EntityActivation,
        parameters: TypedSchemaValue,
        expected_definition: Option<Tool>,
        presented_definition: Option<Tool>,
        next_effective_definition: Tool,
        compatibility: Option<CompiledToolCompatibility>,
    },
    Tool {
        activation: EntityActivation,
    },
}

impl EntityInvocationPlanLayer {
    pub fn activation(&self) -> &EntityActivation {
        match self {
            Self::Middleware { activation, .. } | Self::Tool { activation } => activation,
        }
    }
}

/// Durable location of an entity invocation in a pinned chain plan.
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
pub enum EntityInvocationPlanReference {
    Root {
        plan: EntityInvocationPlan,
    },
    Descendant {
        root_start_index: OplogIndex,
        position: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
pub struct ToolInvocationDescriptor {
    pub attempt_ordinal: u64,
    pub command_path: Vec<String>,
    pub args: Vec<crate::model::card::ToolArgPattern>,
    pub has_stdin: bool,
    pub has_stdout: bool,
    pub declares_stdout: bool,
    pub output_contract: ToolOutputContract,
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
pub struct ToolOutputContract {
    pub result: Option<crate::schema::SchemaGraph>,
    pub errors: Vec<NamedToolErrorSchema>,
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
pub struct NamedToolErrorSchema {
    pub name: String,
    pub payload: crate::schema::SchemaGraph,
}

/// Activation-independent identity used to claim an entity invocation `Start` during historical
/// replay. Rendered arguments are intentionally excluded because they are derived from the pinned
/// activation stored in the claimed request.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityInvocationRequestIdentity {
    pub entity: AgentEntity,
    pub calling_principal: CallingAgentPrincipal,
    pub call_mode: EntityCallMode,
    pub operation: EntityInvocationDescriptorIdentity,
    pub plan_position: Option<EntityInvocationPlanPositionIdentity>,
    pub input: TypedSchemaValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityInvocationPlanPositionIdentity {
    pub root_start_index: OplogIndex,
    pub position: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EntityInvocationDescriptorIdentity {
    Tool(ToolInvocationDescriptorIdentity),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolInvocationDescriptorIdentity {
    pub attempt_ordinal: u64,
    pub command_path: Vec<String>,
    pub has_stdin: bool,
    pub has_stdout: bool,
}

/// Stable invocation-attempt identity used while replay has not yet determined whether the live
/// call was accepted as an entity invocation or durably rejected before dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInvocationClaimIdentity {
    pub accepted: Option<EntityInvocationRequestIdentity>,
    pub rejected: ToolInvocationRejectedIdentity,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub enum ToolInputDecodeFailure {
    InvalidSchemaGraph,
    InvalidSchemaValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolInvocationRejectedIdentity {
    pub attempt_ordinal: u64,
    pub tool_name: ToolName,
    pub command_path: Vec<String>,
    pub input: Option<TypedSchemaValue>,
    pub input_decode_failure: Option<ToolInputDecodeFailure>,
    pub has_stdin: bool,
    pub has_stdout: bool,
    pub call_mode: EntityCallMode,
}

impl EntityInvocationRequestIdentity {
    pub fn matches(&self, request: &EntityInvocationRequest, input: &TypedSchemaValue) -> bool {
        self.entity == request.entity
            && self.calling_principal == request.calling_principal
            && self.call_mode == request.call_mode
            && self.operation == (&request.operation).into()
            && self.plan_position
                == match &request.plan {
                    EntityInvocationPlanReference::Root { .. } => None,
                    EntityInvocationPlanReference::Descendant {
                        root_start_index,
                        position,
                    } => Some(EntityInvocationPlanPositionIdentity {
                        root_start_index: *root_start_index,
                        position: *position,
                    }),
                }
            && &self.input == input
    }
}

impl From<&EntityInvocationDescriptor> for EntityInvocationDescriptorIdentity {
    fn from(value: &EntityInvocationDescriptor) -> Self {
        match value {
            EntityInvocationDescriptor::Tool(tool) => Self::Tool(tool.into()),
        }
    }
}

impl From<&ToolInvocationDescriptor> for ToolInvocationDescriptorIdentity {
    fn from(value: &ToolInvocationDescriptor) -> Self {
        Self {
            attempt_ordinal: value.attempt_ordinal,
            command_path: value.command_path.clone(),
            has_stdin: value.has_stdin,
            has_stdout: value.has_stdout,
        }
    }
}

/// Binary owner-oplog request metadata for one entity invocation. The host payload wraps this as
/// opaque bytes because it is an executor control record rather than a guest-facing schema value.
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct EntityInvocationRequest {
    pub entity: AgentEntity,
    pub calling_principal: CallingAgentPrincipal,
    pub call_mode: EntityCallMode,
    pub operation: EntityInvocationDescriptor,
    pub principal: Principal,
    pub plan: EntityInvocationPlanReference,
    pub assume_idempotence: bool,
}

pub type CallingAgentPrincipal = Principal;

#[derive(Clone, Debug)]
struct ResidentEntitySpan(SpanStarted);

impl PartialEq for ResidentEntitySpan {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ResidentEntitySpan {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityInvocationScope {
    invocation_id: EntityInvocationId,
    parent_start_index: OplogIndex,
    activation: Arc<EntityActivation>,
    calling_principal: CallingAgentPrincipal,
    mode: InvocationExecutionMode,
    idempotency_key: IdempotencyKey,
    assume_idempotence: bool,
    logical_key_positions: bool,
    stream_session_idempotency_key: IdempotencyKey,
    /// Resident tracing context restored from the immutable entity invocation `Start`.
    #[serde(skip)]
    span_started: Option<ResidentEntitySpan>,
}

impl EntityInvocationScope {
    pub fn new(
        invocation_id: EntityInvocationId,
        parent_start_index: OplogIndex,
        activation: Arc<EntityActivation>,
        calling_principal: CallingAgentPrincipal,
        mode: InvocationExecutionMode,
        idempotency_key: IdempotencyKey,
        assume_idempotence: bool,
        logical_key_positions: bool,
        stream_session_idempotency_key: IdempotencyKey,
    ) -> Result<Self, String> {
        if parent_start_index == OplogIndex::NONE {
            return Err("Entity invocation parent Start index cannot be zero".to_string());
        }
        if parent_start_index >= invocation_id.start_index() {
            return Err(
                "Entity invocation parent Start index must precede its Start index".to_string(),
            );
        }
        if invocation_id.entity() != &activation.entity() {
            return Err(
                "Entity invocation selector does not match the activation policy".to_string(),
            );
        }
        if let EntityActivationPolicy::Tool {
            mcp_import: Some(import),
            ..
        } = activation.policy()
            && import.source.environment_id != invocation_id.owner_id().environment_id
        {
            return Err("MCP import activation belongs to a different environment".into());
        }
        match &calling_principal {
            Principal::Agent(principal)
                if principal.agent_id == invocation_id.owner_id().agent_id => {}
            _ => {
                return Err(
                    "Entity invocation calling principal must be its owner agent".to_string(),
                );
            }
        }
        Ok(Self {
            invocation_id,
            parent_start_index,
            activation,
            calling_principal,
            mode,
            idempotency_key,
            assume_idempotence,
            logical_key_positions,
            stream_session_idempotency_key,
            span_started: None,
        })
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        self.invocation_id.owner_id()
    }

    pub fn invocation_id(&self) -> &EntityInvocationId {
        &self.invocation_id
    }

    pub fn parent_start_index(&self) -> OplogIndex {
        self.parent_start_index
    }

    pub fn activation(&self) -> &Arc<EntityActivation> {
        &self.activation
    }

    pub fn calling_principal(&self) -> &CallingAgentPrincipal {
        &self.calling_principal
    }

    pub fn mode(&self) -> InvocationExecutionMode {
        self.mode
    }

    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    pub fn assume_idempotence(&self) -> bool {
        self.assume_idempotence
    }

    pub fn logical_key_positions(&self) -> bool {
        self.logical_key_positions
    }

    pub fn stream_session_idempotency_key(&self) -> &IdempotencyKey {
        &self.stream_session_idempotency_key
    }

    pub fn with_span_started(mut self, span_started: SpanStarted) -> Self {
        self.span_started = Some(ResidentEntitySpan(span_started));
        self
    }

    pub fn span_started(&self) -> Option<&SpanStarted> {
        self.span_started.as_ref().map(|span| &span.0)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityInvocationScopeWire {
    invocation_id: EntityInvocationId,
    parent_start_index: OplogIndex,
    activation: Arc<EntityActivation>,
    calling_principal: CallingAgentPrincipal,
    mode: InvocationExecutionMode,
    idempotency_key: IdempotencyKey,
    assume_idempotence: bool,
    logical_key_positions: bool,
    stream_session_idempotency_key: IdempotencyKey,
}

impl<'de> Deserialize<'de> for EntityInvocationScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = EntityInvocationScopeWire::deserialize(deserializer)?;
        Self::new(
            wire.invocation_id,
            wire.parent_start_index,
            wire.activation,
            wire.calling_principal,
            wire.mode,
            wire.idempotency_key,
            wire.assume_idempotence,
            wire.logical_key_positions,
            wire.stream_session_idempotency_key,
        )
        .map_err(D::Error::custom)
    }
}

impl From<AgentEntity> for golem_api_grpc::proto::golem::worker::AgentEntity {
    fn from(value: AgentEntity) -> Self {
        use golem_api_grpc::proto::golem::worker::agent_entity::Value;

        let value = match value {
            AgentEntity::Tool(name) => Value::ToolName(name.into_inner()),
            AgentEntity::ToolMiddleware(name) => Value::ToolMiddlewareName(name.into_inner()),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentEntity> for AgentEntity {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentEntity,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::agent_entity::Value;

        match value.value.ok_or("Missing AgentEntity.value")? {
            Value::ToolName(name) => ToolName::try_from(name).map(Self::Tool),
            Value::ToolMiddlewareName(name) => {
                ToolMiddlewareName::try_from(name).map(Self::ToolMiddleware)
            }
        }
    }
}

impl From<OwnerRuntime> for golem_api_grpc::proto::golem::worker::OwnerRuntime {
    fn from(value: OwnerRuntime) -> Self {
        use golem_api_grpc::proto::golem::worker::owner_runtime::Value;

        let value = match value {
            OwnerRuntime::Agent => Value::Agent(golem_api_grpc::proto::golem::common::Empty {}),
            OwnerRuntime::Entity(entity) => Value::Entity(entity.into()),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OwnerRuntime> for OwnerRuntime {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::OwnerRuntime,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::owner_runtime::Value;

        match value.value.ok_or("Missing OwnerRuntime.value")? {
            Value::Agent(_) => Ok(Self::Agent),
            Value::Entity(entity) => entity.try_into().map(Self::Entity),
        }
    }
}

impl From<OwnedAgentEntityId> for golem_api_grpc::proto::golem::worker::OwnedAgentEntityId {
    fn from(value: OwnedAgentEntityId) -> Self {
        Self {
            environment_id: Some(value.owner.environment_id.into()),
            owner_agent_id: Some(value.owner.agent_id.into()),
            entity: Some(value.entity.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OwnedAgentEntityId> for OwnedAgentEntityId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::OwnedAgentEntityId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            owner: OwnedAgentId {
                environment_id: value
                    .environment_id
                    .ok_or("Missing OwnedAgentEntityId.environment_id")?
                    .try_into()?,
                agent_id: value
                    .owner_agent_id
                    .ok_or("Missing OwnedAgentEntityId.owner_agent_id")?
                    .try_into()?,
            },
            entity: value
                .entity
                .ok_or("Missing OwnedAgentEntityId.entity")?
                .try_into()?,
        })
    }
}

impl From<EntityInvocationId> for golem_api_grpc::proto::golem::worker::EntityInvocationId {
    fn from(value: EntityInvocationId) -> Self {
        Self {
            entity_id: Some(value.entity_id.into()),
            start_index: value.start_index.as_u64(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::EntityInvocationId> for EntityInvocationId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::EntityInvocationId,
    ) -> Result<Self, Self::Error> {
        Self::new(
            value
                .entity_id
                .ok_or("Missing EntityInvocationId.entity_id")?
                .try_into()?,
            OplogIndex::from_u64(value.start_index),
        )
    }
}

impl From<ExecutableTarget> for golem_api_grpc::proto::golem::worker::ExecutableTarget {
    fn from(value: ExecutableTarget) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.into(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::ExecutableTarget> for ExecutableTarget {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::ExecutableTarget,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing ExecutableTarget.component_id")?
                .try_into()?,
            component_revision: ComponentRevision::try_from(value.component_revision)?,
        })
    }
}

impl From<FilesystemCapability> for golem_api_grpc::proto::golem::worker::FilesystemCapability {
    fn from(value: FilesystemCapability) -> Self {
        match value {
            FilesystemCapability::Capable => Self::Capable,
            FilesystemCapability::Incapable => Self::Incapable,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::FilesystemCapability> for FilesystemCapability {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::FilesystemCapability,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::FilesystemCapability as Proto;

        match value {
            Proto::Capable => Ok(Self::Capable),
            Proto::Incapable => Ok(Self::Incapable),
            Proto::Unspecified => Err("Unspecified FilesystemCapability".to_string()),
        }
    }
}

impl From<EntityActivationPolicy> for golem_api_grpc::proto::golem::worker::EntityActivationPolicy {
    fn from(value: EntityActivationPolicy) -> Self {
        use golem_api_grpc::proto::golem::worker::entity_activation_policy::Value;

        let value = match value {
            EntityActivationPolicy::Tool {
                provision,
                binding,
                mcp_import,
            } => Value::Tool(
                golem_api_grpc::proto::golem::worker::ToolEntityActivationPolicy {
                    provision: Some(provision.into()),
                    binding: Some((*binding).into()),
                    mcp_import: mcp_import.map(|import| {
                        golem_api_grpc::proto::golem::worker::McpImportActivation {
                            environment_id: Some(import.source.environment_id.into()),
                            deployment_revision: import.source.deployment_revision.into(),
                            import_index: import.source.import_index,
                            upstream_tool_name: import.source.upstream_tool_name,
                            protocol_version: import.protocol_version,
                            projected_tool: import.projected_tool,
                        }
                    }),
                },
            ),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name,
                provision,
                config_keys_readable,
                secret_keys_readable,
                secret_keys_revealable,
                filesystem_access,
            } => Value::ToolMiddleware(
                golem_api_grpc::proto::golem::worker::ToolMiddlewareEntityActivationPolicy {
                    middleware_name: middleware_name.into_inner(),
                    provision: Some(provision.into()),
                    config_keys_readable: Some(config_keys_readable.into()),
                    secret_keys_readable: Some(secret_keys_readable.into()),
                    secret_keys_revealable: Some(secret_keys_revealable.into()),
                    filesystem_access:
                        golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::from(
                            filesystem_access,
                        ) as i32,
                },
            ),
        };
        Self { value: Some(value) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::EntityActivationPolicy>
    for EntityActivationPolicy
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::EntityActivationPolicy,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::entity_activation_policy::Value;

        match value.value.ok_or("Missing EntityActivationPolicy.value")? {
            Value::Tool(tool) => Ok(Self::Tool {
                provision: tool
                    .provision
                    .ok_or("Missing ToolEntityActivationPolicy.provision")?
                    .try_into()?,
                binding: Box::new(
                    tool.binding
                        .ok_or("Missing ToolEntityActivationPolicy.binding")?
                        .try_into()?,
                ),
                mcp_import: tool
                    .mcp_import
                    .map(|import| -> Result<_, String> {
                        Ok(Box::new(McpImportActivation {
                            source: crate::model::mcp_import::McpImportSource {
                                environment_id: import
                                    .environment_id
                                    .ok_or("Missing MCP environment")?
                                    .try_into()?,
                                deployment_revision: import.deployment_revision.try_into()?,
                                import_index: import.import_index,
                                upstream_tool_name: import.upstream_tool_name,
                            },
                            protocol_version: import.protocol_version,
                            projected_tool: import.projected_tool,
                        }))
                    })
                    .transpose()?,
            }),
            Value::ToolMiddleware(middleware) => Ok(Self::ToolMiddleware {
                middleware_name: ToolMiddlewareName::try_from(middleware.middleware_name)?,
                provision: middleware
                    .provision
                    .ok_or("Missing ToolMiddlewareEntityActivationPolicy.provision")?
                    .try_into()?,
                config_keys_readable: middleware
                    .config_keys_readable
                    .ok_or("Missing ToolMiddlewareEntityActivationPolicy.config_keys_readable")?
                    .try_into()?,
                secret_keys_readable: middleware
                    .secret_keys_readable
                    .ok_or("Missing ToolMiddlewareEntityActivationPolicy.secret_keys_readable")?
                    .try_into()?,
                secret_keys_revealable: middleware
                    .secret_keys_revealable
                    .ok_or("Missing ToolMiddlewareEntityActivationPolicy.secret_keys_revealable")?
                    .try_into()?,
                filesystem_access:
                    golem_api_grpc::proto::golem::registry::ToolFilesystemAccess::try_from(
                        middleware.filesystem_access,
                    )
                    .map_err(|error| error.to_string())?
                    .into(),
            }),
        }
    }
}

impl From<EntityActivationFingerprint>
    for golem_api_grpc::proto::golem::worker::EntityActivationFingerprint
{
    fn from(value: EntityActivationFingerprint) -> Self {
        Self {
            value: value.0.to_vec(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::EntityActivationFingerprint>
    for EntityActivationFingerprint
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::EntityActivationFingerprint,
    ) -> Result<Self, Self::Error> {
        let bytes: [u8; 32] = value.value.try_into().map_err(|value: Vec<u8>| {
            format!(
                "Invalid EntityActivationFingerprint length: expected 32, got {}",
                value.len()
            )
        })?;
        Ok(Self(bytes))
    }
}

impl From<EntityActivation> for golem_api_grpc::proto::golem::worker::EntityActivation {
    fn from(value: EntityActivation) -> Self {
        use golem_api_grpc::proto::golem::worker::entity_activation::Source;

        let source = match value.source {
            EntityActivationSource::Component { executable } => {
                Source::Component(executable.into())
            }
            EntityActivationSource::Host {
                host_tool_id,
                implementation_version,
            } => Source::Host(
                golem_api_grpc::proto::golem::worker::HostEntityActivationSource {
                    host_tool_id: host_tool_id.into(),
                    implementation_version,
                },
            ),
        };
        Self {
            source: Some(source),
            deployment_revision: value.deployment_revision.into(),
            policy: Some(value.policy.into()),
            filesystem: golem_api_grpc::proto::golem::worker::FilesystemCapability::from(
                value.filesystem,
            ) as i32,
            fingerprint: Some(value.fingerprint.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::EntityActivation> for EntityActivation {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::EntityActivation,
    ) -> Result<Self, Self::Error> {
        let filesystem =
            golem_api_grpc::proto::golem::worker::FilesystemCapability::try_from(value.filesystem)
                .map_err(|_| format!("Invalid EntityActivation.filesystem: {}", value.filesystem))?
                .try_into()?;
        use golem_api_grpc::proto::golem::worker::entity_activation::Source;

        let source = match value.source.ok_or("Missing EntityActivation.source")? {
            Source::Component(executable) => EntityActivationSource::Component {
                executable: executable.try_into()?,
            },
            Source::Host(host) => EntityActivationSource::Host {
                host_tool_id: HostToolId::try_from(host.host_tool_id)?,
                implementation_version: host.implementation_version,
            },
        };
        let deployment_revision = DeploymentRevision::try_from(value.deployment_revision)?;
        let policy = value
            .policy
            .ok_or("Missing EntityActivation.policy")?
            .try_into()?;
        let fingerprint: EntityActivationFingerprint = value
            .fingerprint
            .ok_or("Missing EntityActivation.fingerprint")?
            .try_into()?;
        let activation = Self::new_with_source(source, deployment_revision, policy, filesystem)?;
        if fingerprint != activation.fingerprint {
            return Err("EntityActivation fingerprint does not match its contents".to_string());
        }
        Ok(activation)
    }
}

impl From<InvocationExecutionMode>
    for golem_api_grpc::proto::golem::worker::InvocationExecutionMode
{
    fn from(value: InvocationExecutionMode) -> Self {
        match value {
            InvocationExecutionMode::Live => Self::Live,
            InvocationExecutionMode::ReplayingCompleted => Self::ReplayingCompleted,
            InvocationExecutionMode::ReplayingIncomplete => Self::ReplayingIncomplete,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::InvocationExecutionMode>
    for InvocationExecutionMode
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::InvocationExecutionMode,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::InvocationExecutionMode as Proto;

        match value {
            Proto::Live => Ok(Self::Live),
            Proto::ReplayingCompleted => Ok(Self::ReplayingCompleted),
            Proto::ReplayingIncomplete => Ok(Self::ReplayingIncomplete),
            Proto::Unspecified => Err("Unspecified InvocationExecutionMode".to_string()),
        }
    }
}

impl From<EntityInvocationScope> for golem_api_grpc::proto::golem::worker::EntityInvocationScope {
    fn from(value: EntityInvocationScope) -> Self {
        Self {
            invocation_id: Some(value.invocation_id.into()),
            parent_start_index: value.parent_start_index.as_u64(),
            activation: Some(Arc::unwrap_or_clone(value.activation).into()),
            calling_principal: Some(value.calling_principal.into()),
            mode: golem_api_grpc::proto::golem::worker::InvocationExecutionMode::from(value.mode)
                as i32,
            idempotency_key: Some(value.idempotency_key.into()),
            assume_idempotence: value.assume_idempotence,
            logical_key_positions: value.logical_key_positions,
            stream_session_idempotency_key: Some(value.stream_session_idempotency_key.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::EntityInvocationScope>
    for EntityInvocationScope
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::EntityInvocationScope,
    ) -> Result<Self, Self::Error> {
        let mode =
            golem_api_grpc::proto::golem::worker::InvocationExecutionMode::try_from(value.mode)
                .map_err(|_| format!("Invalid EntityInvocationScope.mode: {}", value.mode))?
                .try_into()?;
        Self::new(
            value
                .invocation_id
                .ok_or("Missing EntityInvocationScope.invocation_id")?
                .try_into()?,
            OplogIndex::from_u64(value.parent_start_index),
            Arc::new(
                value
                    .activation
                    .ok_or("Missing EntityInvocationScope.activation")?
                    .try_into()?,
            ),
            value
                .calling_principal
                .ok_or("Missing EntityInvocationScope.calling_principal")?
                .try_into()?,
            mode,
            value
                .idempotency_key
                .ok_or("Missing EntityInvocationScope.idempotency_key")?
                .into(),
            value.assume_idempotence,
            value.logical_key_positions,
            value
                .stream_session_idempotency_key
                .ok_or("Missing EntityInvocationScope.stream_session_idempotency_key")?
                .into(),
        )
    }
}
