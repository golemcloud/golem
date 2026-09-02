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
use crate::model::OwnedAgentId;
use crate::model::component::{ComponentId, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::model::oplog::OplogIndex;
use crate::model::tool::{
    CompiledToolBinding, SecretKeyScope, ToolFilesystemAccess, ToolName, ToolProvisionConfig,
};
use crate::schema::TypedSchemaValue;
use desert_rust::BinaryCodec;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

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
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EntityActivationPolicy {
    Tool {
        provision: ToolProvisionConfig,
        binding: Box<CompiledToolBinding>,
    },
    ToolMiddleware {
        middleware_name: ToolMiddlewareName,
        provision: ToolProvisionConfig,
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
    executable: ExecutableTarget,
    deployment_revision: DeploymentRevision,
    policy: EntityActivationPolicy,
    filesystem: FilesystemCapability,
    fingerprint: EntityActivationFingerprint,
}

#[derive(BinaryCodec)]
#[desert(evolution())]
struct EntityActivationFingerprintInput {
    executable: ExecutableTarget,
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
        Self::validate(&executable, deployment_revision, &policy, filesystem)?;
        let fingerprint_input = EntityActivationFingerprintInput {
            executable: executable.clone(),
            deployment_revision,
            policy: policy.clone(),
            filesystem,
        };
        let bytes = desert_rust::serialize_to_byte_vec(&fingerprint_input)
            .map_err(|error| format!("Failed to fingerprint entity activation: {error}"))?;
        let fingerprint = EntityActivationFingerprint::from_bytes(*blake3::hash(&bytes).as_bytes());

        Ok(Self {
            executable,
            deployment_revision,
            policy,
            filesystem,
            fingerprint,
        })
    }

    fn validate(
        executable: &ExecutableTarget,
        deployment_revision: DeploymentRevision,
        policy: &EntityActivationPolicy,
        filesystem: FilesystemCapability,
    ) -> Result<(), String> {
        match policy {
            EntityActivationPolicy::Tool { provision, binding } => {
                if binding.deployment_revision != deployment_revision {
                    return Err(
                        "Entity activation and tool binding deployment revisions differ"
                            .to_string(),
                    );
                }
                let crate::model::tool::ToolSource::Component {
                    component_id,
                    component_revision,
                    ..
                } = &binding.source;
                if *component_id != executable.component_id
                    || *component_revision != executable.component_revision
                {
                    return Err(
                        "Entity executable does not match the tool binding source".to_string()
                    );
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

    pub fn executable(&self) -> &ExecutableTarget {
        &self.executable
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
    executable: ExecutableTarget,
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
        let activation = Self::new(
            wire.executable,
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

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
pub struct ToolInvocationDescriptor {
    pub attempt_ordinal: u64,
    pub command_path: Vec<String>,
    pub args: Vec<String>,
    pub has_stdin: bool,
    pub has_stdout: bool,
    pub declares_stdout: bool,
}

/// Activation-independent identity used to claim an entity invocation `Start` during historical
/// replay. Rendered arguments are intentionally excluded because they are derived from the pinned
/// activation stored in the claimed request.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityInvocationRequestIdentity {
    pub entity: AgentEntity,
    pub calling_principal: CallingAgentPrincipal,
    pub call_mode: EntityCallMode,
    pub operation: Option<EntityInvocationDescriptorIdentity>,
    pub input: TypedSchemaValue,
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
            && self.operation == request.operation.as_ref().map(Into::into)
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
#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
#[desert(evolution(
    FieldAdded("operation", None::<EntityInvocationDescriptor>),
    FieldAdded("principal", None::<Principal>)
))]
pub struct EntityInvocationRequest {
    pub entity: AgentEntity,
    pub activation: EntityActivation,
    pub calling_principal: CallingAgentPrincipal,
    pub call_mode: EntityCallMode,
    pub operation: Option<EntityInvocationDescriptor>,
    pub principal: Option<Principal>,
}

pub type CallingAgentPrincipal = Principal;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityInvocationScope {
    invocation_id: EntityInvocationId,
    parent_start_index: OplogIndex,
    activation: Arc<EntityActivation>,
    calling_principal: CallingAgentPrincipal,
    mode: InvocationExecutionMode,
}

impl EntityInvocationScope {
    pub fn new(
        invocation_id: EntityInvocationId,
        parent_start_index: OplogIndex,
        activation: Arc<EntityActivation>,
        calling_principal: CallingAgentPrincipal,
        mode: InvocationExecutionMode,
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityInvocationScopeWire {
    invocation_id: EntityInvocationId,
    parent_start_index: OplogIndex,
    activation: Arc<EntityActivation>,
    calling_principal: CallingAgentPrincipal,
    mode: InvocationExecutionMode,
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
            EntityActivationPolicy::Tool { provision, binding } => Value::Tool(
                golem_api_grpc::proto::golem::worker::ToolEntityActivationPolicy {
                    provision: Some(provision.into()),
                    binding: Some((*binding).into()),
                },
            ),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name,
                provision,
                secret_keys_readable,
                secret_keys_revealable,
                filesystem_access,
            } => Value::ToolMiddleware(
                golem_api_grpc::proto::golem::worker::ToolMiddlewareEntityActivationPolicy {
                    middleware_name: middleware_name.into_inner(),
                    provision: Some(provision.into()),
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
            }),
            Value::ToolMiddleware(middleware) => Ok(Self::ToolMiddleware {
                middleware_name: ToolMiddlewareName::try_from(middleware.middleware_name)?,
                provision: middleware
                    .provision
                    .ok_or("Missing ToolMiddlewareEntityActivationPolicy.provision")?
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
        Self {
            executable: Some(value.executable.into()),
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
        let executable = value
            .executable
            .ok_or("Missing EntityActivation.executable")?
            .try_into()?;
        let deployment_revision = DeploymentRevision::try_from(value.deployment_revision)?;
        let policy = value
            .policy
            .ok_or("Missing EntityActivation.policy")?
            .try_into()?;
        let fingerprint: EntityActivationFingerprint = value
            .fingerprint
            .ok_or("Missing EntityActivation.fingerprint")?
            .try_into()?;
        let activation = Self::new(executable, deployment_revision, policy, filesystem)?;
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
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentId;
    use crate::model::account::{AccountEmail, AccountId};
    use crate::model::agent::{AgentPrincipal, AgentTypeName, GolemUserPrincipal};
    use crate::model::component::ComponentName;
    use crate::model::environment::EnvironmentId;
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::{SecretKeyScope, ToolSource};
    use test_r::test;

    fn owner() -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Example(\"owner\")".to_string(),
            },
        )
    }

    fn activation() -> EntityActivation {
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::try_from(7_u64).unwrap();
        let deployment_revision = DeploymentRevision::try_from(11_u64).unwrap();
        let source = ToolSource::Component {
            component_id,
            component_revision,
            component_name: ComponentName("tools:search".to_string()),
        };
        let binding = CompiledToolBinding {
            deployment_revision,
            agent_type_name: AgentTypeName("Example".to_string()),
            tool_name: ToolName::try_from("search").unwrap(),
            version: "1.0.0".to_string(),
            metadata_version: "0.1.0".to_string(),
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: crate::model::tool::ToolFilesystemAccess::Unset,
            source,
        };

        EntityActivation::new(
            ExecutableTarget::new(component_id, component_revision),
            deployment_revision,
            EntityActivationPolicy::Tool {
                provision: ToolProvisionConfig::default(),
                binding: Box::new(binding),
            },
            FilesystemCapability::Incapable,
        )
        .unwrap()
    }

    fn middleware_activation() -> EntityActivation {
        EntityActivation::new(
            ExecutableTarget::new(
                ComponentId::new(),
                ComponentRevision::try_from(9_u64).unwrap(),
            ),
            DeploymentRevision::try_from(12_u64).unwrap(),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name: ToolMiddlewareName::try_from("audit").unwrap(),
                provision: ToolProvisionConfig::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
            },
            FilesystemCapability::Incapable,
        )
        .unwrap()
    }

    #[test]
    fn equal_tool_and_middleware_names_are_distinct_selectors() {
        let tool = AgentEntity::Tool(ToolName::try_from("search").unwrap());
        let middleware =
            AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("search").unwrap());

        assert_ne!(tool, middleware);
    }

    #[test]
    fn entity_ids_project_to_the_unchanged_owner() {
        let owner = owner();
        let entity_id = OwnedAgentEntityId {
            owner: owner.clone(),
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        };
        let invocation_id =
            EntityInvocationId::new(entity_id.clone(), OplogIndex::from_u64(42)).unwrap();

        assert_eq!(entity_id.owner_id(), &owner);
        assert_eq!(invocation_id.owner_id(), &owner);
        assert_eq!(invocation_id.start_index(), OplogIndex::from_u64(42));
    }

    #[test]
    fn entity_invocation_id_json_roundtrip_is_structured() {
        let invocation_id = EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner(),
                entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            },
            OplogIndex::from_u64(42),
        )
        .unwrap();

        let json = serde_json::to_value(&invocation_id).unwrap();
        let decoded: EntityInvocationId = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(decoded, invocation_id);
        assert_eq!(json["entityId"]["entity"]["kind"], "tool");
        assert_eq!(json["entityId"]["entity"]["name"], "search");
        assert_eq!(json["startIndex"], 42);
    }

    #[test]
    fn entity_invocation_id_protobuf_roundtrip_is_structured() {
        let invocation_id = EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner(),
                entity: AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap()),
            },
            OplogIndex::from_u64(84),
        )
        .unwrap();

        let protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationId =
            invocation_id.clone().into();
        let decoded: EntityInvocationId = protobuf.try_into().unwrap();

        assert_eq!(decoded, invocation_id);
    }

    #[test]
    fn entity_invocation_request_binary_roundtrip_preserves_activation() {
        let owner = owner();
        let request = EntityInvocationRequest {
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            activation: activation(),
            calling_principal: Principal::Agent(AgentPrincipal {
                agent_id: owner.agent_id,
            }),
            call_mode: EntityCallMode::Asynchronous,
            operation: Some(EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
                attempt_ordinal: 7,
                command_path: vec!["files".to_string(), "search".to_string()],
                args: vec!["--ignore-case".to_string(), "needle".to_string()],
                has_stdin: true,
                has_stdout: true,
                declares_stdout: true,
            })),
            principal: Some(Principal::GolemUser(GolemUserPrincipal {
                account_id: AccountId::new(),
            })),
        };

        let bytes = desert_rust::serialize_to_byte_vec(&request).unwrap();
        let decoded: EntityInvocationRequest = desert_rust::deserialize(&bytes).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn entity_invocation_claim_identity_ignores_pinned_dispatch_derivations_only() {
        let owner = owner();
        let input = TypedSchemaValue::new(
            crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(Vec::new())),
            crate::schema::SchemaValue::Tuple {
                elements: Vec::new(),
            },
        );
        let request = EntityInvocationRequest {
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            activation: activation(),
            calling_principal: Principal::Agent(AgentPrincipal {
                agent_id: owner.agent_id,
            }),
            call_mode: EntityCallMode::Asynchronous,
            operation: Some(EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
                attempt_ordinal: 7,
                command_path: vec!["files".to_string(), "search".to_string()],
                args: vec!["--recorded-rendering".to_string()],
                has_stdin: true,
                has_stdout: false,
                declares_stdout: false,
            })),
            principal: None,
        };
        let identity = EntityInvocationRequestIdentity {
            entity: request.entity.clone(),
            calling_principal: request.calling_principal.clone(),
            call_mode: request.call_mode,
            operation: request.operation.as_ref().map(Into::into),
            input: input.clone(),
        };
        let mut differently_pinned = request.clone();
        differently_pinned.activation = activation();
        if let Some(EntityInvocationDescriptor::Tool(descriptor)) =
            differently_pinned.operation.as_mut()
        {
            descriptor.args = vec!["--new-rendering".to_string()];
            descriptor.declares_stdout = true;
        }

        assert!(identity.matches(&differently_pinned, &input));

        if let Some(EntityInvocationDescriptor::Tool(descriptor)) =
            differently_pinned.operation.as_mut()
        {
            descriptor.attempt_ordinal = 8;
        }
        assert!(!identity.matches(&differently_pinned, &input));
        if let Some(EntityInvocationDescriptor::Tool(descriptor)) =
            differently_pinned.operation.as_mut()
        {
            descriptor.attempt_ordinal = 7;
        }

        if let Some(EntityInvocationDescriptor::Tool(descriptor)) =
            differently_pinned.operation.as_mut()
        {
            descriptor.command_path.push("other".to_string());
        }
        assert!(!identity.matches(&differently_pinned, &input));
        if let Some(EntityInvocationDescriptor::Tool(descriptor)) =
            differently_pinned.operation.as_mut()
        {
            descriptor.command_path.pop();
            descriptor.has_stdout = true;
        }
        assert!(!identity.matches(&differently_pinned, &input));

        let different_input = TypedSchemaValue::new(
            crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(vec![
                crate::schema::SchemaType::bool(),
            ])),
            crate::schema::SchemaValue::Tuple {
                elements: vec![crate::schema::SchemaValue::Bool(true)],
            },
        );
        assert!(!identity.matches(&request, &different_input));
    }

    #[test]
    fn legacy_entity_invocation_request_decodes_without_operation_descriptor() {
        #[derive(BinaryCodec)]
        #[desert(evolution())]
        struct LegacyEntityInvocationRequest {
            entity: AgentEntity,
            activation: EntityActivation,
            calling_principal: CallingAgentPrincipal,
            call_mode: EntityCallMode,
        }

        let owner = owner();
        let legacy = LegacyEntityInvocationRequest {
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            activation: activation(),
            calling_principal: Principal::Agent(AgentPrincipal {
                agent_id: owner.agent_id,
            }),
            call_mode: EntityCallMode::Synchronous,
        };
        let bytes = desert_rust::serialize_to_byte_vec(&legacy).unwrap();
        let decoded: EntityInvocationRequest = desert_rust::deserialize(&bytes).unwrap();

        assert_eq!(decoded.entity, legacy.entity);
        assert_eq!(decoded.activation, legacy.activation);
        assert_eq!(decoded.calling_principal, legacy.calling_principal);
        assert_eq!(decoded.call_mode, legacy.call_mode);
        assert_eq!(decoded.operation, None);
        assert_eq!(decoded.principal, None);
    }

    #[test]
    fn invocation_scope_protobuf_roundtrip_preserves_activation_fingerprint() {
        let owner = owner();
        let scope = EntityInvocationScope::new(
            EntityInvocationId::new(
                OwnedAgentEntityId {
                    owner: owner.clone(),
                    entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
                },
                OplogIndex::from_u64(84),
            )
            .unwrap(),
            OplogIndex::from_u64(81),
            Arc::new(activation()),
            Principal::Agent(AgentPrincipal {
                agent_id: owner.agent_id,
            }),
            InvocationExecutionMode::ReplayingCompleted,
        )
        .unwrap();

        let protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationScope =
            scope.clone().into();
        let decoded: EntityInvocationScope = protobuf.try_into().unwrap();

        assert_eq!(decoded, scope);
    }

    #[test]
    fn middleware_invocation_scope_roundtrips_through_binary_and_protobuf() {
        let owner = owner();
        let activation = Arc::new(middleware_activation());
        let scope = EntityInvocationScope::new(
            EntityInvocationId::new(
                OwnedAgentEntityId {
                    owner: owner.clone(),
                    entity: AgentEntity::ToolMiddleware(
                        ToolMiddlewareName::try_from("audit").unwrap(),
                    ),
                },
                OplogIndex::from_u64(91),
            )
            .unwrap(),
            OplogIndex::from_u64(84),
            activation.clone(),
            Principal::Agent(AgentPrincipal {
                agent_id: owner.agent_id.clone(),
            }),
            InvocationExecutionMode::ReplayingIncomplete,
        )
        .unwrap();
        let request = EntityInvocationRequest {
            entity: scope.invocation_id().entity().clone(),
            activation: activation.as_ref().clone(),
            calling_principal: scope.calling_principal().clone(),
            call_mode: EntityCallMode::Synchronous,
            operation: None,
            principal: None,
        };

        let request_bytes = desert_rust::serialize_to_byte_vec(&request).unwrap();
        assert_eq!(
            desert_rust::deserialize::<EntityInvocationRequest>(&request_bytes).unwrap(),
            request
        );
        let protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationScope =
            scope.clone().into();
        assert_eq!(EntityInvocationScope::try_from(protobuf).unwrap(), scope);
    }

    #[test]
    fn activation_protobuf_rejects_content_that_does_not_match_fingerprint() {
        let activation = activation();
        let mut protobuf: golem_api_grpc::proto::golem::worker::EntityActivation =
            activation.into();
        protobuf.fingerprint.as_mut().unwrap().value[0] ^= 1;

        let result = EntityActivation::try_from(protobuf);

        assert_eq!(
            result.unwrap_err(),
            "EntityActivation fingerprint does not match its contents"
        );
    }

    #[test]
    fn activation_json_rejects_content_that_does_not_match_fingerprint() {
        let mut json = serde_json::to_value(activation()).unwrap();
        json["fingerprint"][0] = serde_json::json!(255);

        let result = serde_json::from_value::<EntityActivation>(json);

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("fingerprint does not match")
        );
    }

    #[test]
    fn malformed_tool_middleware_name_is_rejected_by_json() {
        let result = serde_json::from_str::<AgentEntity>(
            r#"{"kind":"toolMiddleware","name":"Not Kebab Case"}"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn zero_entity_invocation_start_index_is_rejected_by_protobuf() {
        let entity_id = OwnedAgentEntityId {
            owner: owner(),
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        };
        let protobuf = golem_api_grpc::proto::golem::worker::EntityInvocationId {
            entity_id: Some(entity_id.into()),
            start_index: 0,
        };

        let result = EntityInvocationId::try_from(protobuf);

        assert_eq!(
            result.unwrap_err(),
            "Entity invocation Start index cannot be zero"
        );
    }

    #[test]
    fn invocation_scope_rejects_parent_that_does_not_precede_invocation_start() {
        let owner = owner();
        let activation = Arc::new(activation());
        let entity_id = OwnedAgentEntityId {
            owner: owner.clone(),
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        };
        let principal = Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id,
        });

        for parent_start_index in [OplogIndex::from_u64(42), OplogIndex::from_u64(43)] {
            let scope = EntityInvocationScope::new(
                EntityInvocationId::new(entity_id.clone(), OplogIndex::from_u64(42)).unwrap(),
                parent_start_index,
                activation.clone(),
                principal.clone(),
                InvocationExecutionMode::Live,
            );

            assert!(
                scope.is_err(),
                "a durable parent Start must precede its nested entity invocation Start"
            );
        }
    }

    #[test]
    fn missing_owner_component_id_is_rejected_by_protobuf() {
        let protobuf = golem_api_grpc::proto::golem::worker::OwnedAgentEntityId {
            environment_id: Some(EnvironmentId::new().into()),
            owner_agent_id: Some(golem_api_grpc::proto::golem::worker::AgentId {
                component_id: None,
                name: "Example(\"owner\")".to_string(),
            }),
            entity: Some(AgentEntity::Tool(ToolName::try_from("search").unwrap()).into()),
        };

        let result = OwnedAgentEntityId::try_from(protobuf);

        assert_eq!(result.unwrap_err(), "Missing AgentId.component_id");
    }

    #[test]
    fn activation_rejects_executable_that_differs_from_binding_source() {
        let activation = activation();
        let result = EntityActivation::new(
            ExecutableTarget::new(ComponentId::new(), activation.executable.component_revision),
            activation.deployment_revision,
            activation.policy,
            activation.filesystem,
        );

        assert_eq!(
            result.unwrap_err(),
            "Entity executable does not match the tool binding source"
        );
    }
}
