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

use super::DesignatedKind;
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::application::ApplicationId;
use crate::base_model::durable_stream::StreamId;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, AgentId, InvocationId, PromiseId};
use serde::{Deserialize, Serialize};

/// The governed thing that one part of a request names: the owner side of a grant.
///
/// A part that acts on an entity (an invocation, a task, a stream, a promise, a webhook, an
/// artifact or a package) targets that entity, together with the entity's owner, against which a
/// grant's owner is matched.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase", tag = "kind", content = "target")]
pub enum Target {
    Agent(AgentTarget),
    Tool(ToolTarget),
    Invocation(InvocationTarget),
    Task(EntityTarget),
    Stream(StreamTarget),
    Promise(PromiseTarget),
    Webhook(EntityTarget),
    Artifact(EntityTarget),
    Package(EntityTarget),
    Account(AccountTarget),
    Application(ApplicationTarget),
    Environment(EnvironmentTarget),
}

/// The kind of a [`Target`], without its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    Agent,
    Tool,
    Invocation,
    Task,
    Stream,
    Promise,
    Webhook,
    Artifact,
    Package,
    Account,
    Application,
    Environment,
}

impl Target {
    pub fn kind(&self) -> TargetKind {
        match self {
            Target::Agent(_) => TargetKind::Agent,
            Target::Tool(_) => TargetKind::Tool,
            Target::Invocation(_) => TargetKind::Invocation,
            Target::Task(_) => TargetKind::Task,
            Target::Stream(_) => TargetKind::Stream,
            Target::Promise(_) => TargetKind::Promise,
            Target::Webhook(_) => TargetKind::Webhook,
            Target::Artifact(_) => TargetKind::Artifact,
            Target::Package(_) => TargetKind::Package,
            Target::Account(_) => TargetKind::Account,
            Target::Application(_) => TargetKind::Application,
            Target::Environment(_) => TargetKind::Environment,
        }
    }

    /// The kind of the entity's owner, for a target that is an entity with an owner.
    pub fn owner_kind(&self) -> Option<OwnerKind> {
        match self {
            Target::Invocation(invocation) => Some(invocation.owner.kind()),
            Target::Task(entity)
            | Target::Webhook(entity)
            | Target::Artifact(entity)
            | Target::Package(entity) => Some(entity.owner.kind()),
            Target::Stream(_) => Some(OwnerKind::Invocation),
            Target::Promise(_) => Some(OwnerKind::Agent),
            Target::Agent(_)
            | Target::Tool(_)
            | Target::Account(_)
            | Target::Application(_)
            | Target::Environment(_) => None,
        }
    }
}

/// The kind of an entity's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerKind {
    Agent,
    Tool,
    Invocation,
    Environment,
    Account,
}

impl InvocationOwner {
    pub fn kind(&self) -> OwnerKind {
        match self {
            InvocationOwner::Agent(_) => OwnerKind::Agent,
            InvocationOwner::Tool(_) => OwnerKind::Tool,
        }
    }
}

impl Owner {
    pub fn kind(&self) -> OwnerKind {
        match self {
            Owner::Agent(_) => OwnerKind::Agent,
            Owner::Tool(_) => OwnerKind::Tool,
            Owner::Invocation(_) => OwnerKind::Invocation,
            Owner::Environment(_) => OwnerKind::Environment,
            Owner::Account(_) => OwnerKind::Account,
        }
    }
}

impl TargetKind {
    /// The kind of "unknown ID" that a denial on this target must look like, or
    /// `None` when a denial on it is a `403`.
    pub fn designated_kind(self) -> Option<DesignatedKind> {
        match self {
            TargetKind::Agent => Some(DesignatedKind::Agent),
            TargetKind::Invocation => Some(DesignatedKind::Invocation),
            TargetKind::Task => Some(DesignatedKind::Task),
            TargetKind::Stream => Some(DesignatedKind::Stream),
            TargetKind::Promise => Some(DesignatedKind::Promise),
            TargetKind::Webhook => Some(DesignatedKind::Webhook),
            TargetKind::Artifact => Some(DesignatedKind::Artifact),
            TargetKind::Tool
            | TargetKind::Package
            | TargetKind::Account
            | TargetKind::Application
            | TargetKind::Environment => None,
        }
    }
}

/// An agent, as its metadata describes it. The deciding service fills every field from the
/// agent's metadata, which it already loads to route or run the request, and never parses them
/// from the agent ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct AgentTarget {
    pub environment_id: EnvironmentId,
    pub agent_type: AgentTypeName,
    pub agent_id: AgentId,
    /// The fingerprint of the agent's current lifetime. A grant that recorded another
    /// fingerprint does not reach it.
    pub fingerprint: AgentFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct ToolTarget {
    pub environment_id: EnvironmentId,
    pub tool_name: ToolName,
}

/// An invocation and its owner: the agent that runs it, or the tool of a standalone tool
/// invocation. An agent-hosted tool invocation belongs to its hosting agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct InvocationTarget {
    pub invocation_id: InvocationId,
    pub owner: InvocationOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase", tag = "kind", content = "owner")]
pub enum InvocationOwner {
    Agent(AgentTarget),
    Tool(ToolTarget),
}

/// A durable stream and the invocation that it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct StreamTarget {
    pub stream_id: StreamId,
    pub owner: InvocationTarget,
}

/// A promise and the agent that created it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct PromiseTarget {
    pub promise_id: PromiseId,
    pub owner: AgentTarget,
}

/// An entity whose ID is known only by its text form here: a webhook, an artifact, a package or
/// an A2A task. The ID is the entity's canonical text form, as a grant names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct EntityTarget {
    pub id: EntityIdText,
    pub owner: Owner,
}

/// The canonical text form of an entity ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct EntityIdText(pub String);

/// The owner of an entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase", tag = "kind", content = "owner")]
pub enum Owner {
    Agent(AgentTarget),
    Tool(ToolTarget),
    Invocation(InvocationTarget),
    Environment(EnvironmentTarget),
    Account(AccountTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct AccountTarget {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct ApplicationTarget {
    pub account_id: AccountId,
    pub application_id: ApplicationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentTarget {
    pub account_id: AccountId,
    pub application_id: ApplicationId,
    pub environment_id: EnvironmentId,
}
