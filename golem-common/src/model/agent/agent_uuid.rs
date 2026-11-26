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

//! Derivations of agent IDs. The bit layout is documented on [`AgentId`].
//!
//! A derived H is the first 90 bits of a SHA-256 digest. Each derivation hashes its
//! own domain tag first, and every variable-length input, the domain tag included,
//! is a `u64` big-endian byte length followed by its bytes. Environment IDs, agent
//! IDs, component IDs and fingerprints are 16 raw bytes and type IDs are 4
//! big-endian bytes.
//!
//! The derivations, their domain tags and the layout are part of every derived
//! agent ID: changing any of them moves every such agent to another address. The
//! golden tests of this module guard them.

use sha2::{Digest, Sha256};

use super::AgentKey;
use crate::base_model::agent_id::H_MASK;
use crate::base_model::component::ComponentId;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::type_id::TypeId;
use crate::base_model::{
    AgentFingerprint, AgentId, IdempotencyKey, IdempotencyScope, RESERVED_TYPE_TAG,
};

const KEYED_DOMAIN: &str = "golem:agent-id:keyed:v1";
const IDEMPOTENT_CREATE_DOMAIN: &str = "golem:agent-id:idempotent-create:v1";
const EPHEMERAL_INVOCATION_DOMAIN: &str = "golem:agent-id:ephemeral-invocation:v1";
const EPHEMERAL_PROXY_DOMAIN: &str = "golem:agent-id:ephemeral-proxy:v1";
const DURABLE_STREAM_FORK_DOMAIN: &str = "golem:agent-id:durable-stream-fork:v1";
const DURABLE_STREAM_SESSION_DOMAIN: &str = "golem:agent-id:durable-stream-session:v1";
const IMPORTED_PHANTOM_DOMAIN: &str = "golem:agent-id:imported-phantom:v1";
const ALLOCATED_DOMAIN: &str = "golem:agent-id:allocated:v1";
const EXTERNAL_TOOL_OWNER_DOMAIN: &str = "golem:agent-id:external-tool-owner:v1";

impl AgentId {
    /// The ID of the keyed agent that `key` selects. `type_id` must be the type ID
    /// of `key.agent_type()`; the type name is not part of the hash.
    pub fn keyed(environment_id: EnvironmentId, type_id: TypeId, key: &AgentKey) -> Self {
        Self::typed(
            type_id,
            HashInput::new(KEYED_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(key.canonical_params().as_bytes()),
        )
    }

    /// The ID of an agent created without a key, under an idempotency key.
    /// A caller without a caller scope never reaches this derivation: its
    /// idempotency key is ignored, and it gets [`Self::random`].
    pub fn idempotent_create(
        environment_id: EnvironmentId,
        type_id: TypeId,
        caller_scope: &IdempotencyScope,
        idempotency_key: &IdempotencyKey,
    ) -> Self {
        Self::typed(
            type_id,
            HashInput::new(IDEMPOTENT_CREATE_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(&caller_scope.encoded())
                .variable(idempotency_key.value.as_bytes()),
        )
    }

    /// The ID of the instance that runs an ephemeral invocation, so a retry reaches the
    /// same instance. `owner_key` is the invocation's owner key
    /// ([`crate::model::InvocationRequestKey::owner_key`]), which binds the caller scope,
    /// the operation and the idempotency key, so the executor can check the ID from the
    /// owner key alone.
    pub fn ephemeral_invocation(
        environment_id: EnvironmentId,
        type_id: TypeId,
        owner_key: &IdempotencyKey,
    ) -> Self {
        Self::typed(
            type_id,
            HashInput::new(EPHEMERAL_INVOCATION_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(owner_key.value.as_bytes()),
        )
    }

    /// The logical identity of an ephemeral RPC proxy, not an instance's address.
    /// Constructor parameters are not a key and have no key-length limit. Each call
    /// through the proxy uses [`Self::ephemeral_invocation`] for its actual target.
    pub fn ephemeral_proxy(
        environment_id: EnvironmentId,
        type_id: TypeId,
        constructor: &super::ParsedAgentKey,
    ) -> Result<Self, String> {
        Ok(Self::typed(
            type_id,
            HashInput::new(EPHEMERAL_PROXY_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(constructor.canonical_parameters()?.as_bytes()),
        ))
    }

    /// The ID of the Durable Streams fork named `fork_name` of `root`. It carries the root's
    /// type tag. The route is not part of the hash: a fork name names one fork of the root
    /// on every route and in every deployment revision.
    pub fn durable_stream_fork(
        environment_id: EnvironmentId,
        root: AgentId,
        fork_name: &str,
    ) -> Self {
        Self::from_parts(
            root.type_tag(),
            HashInput::new(DURABLE_STREAM_FORK_DOMAIN)
                .environment(environment_id)
                .fixed(root.as_uuid().as_bytes())
                .variable(fork_name.as_bytes())
                .h(),
        )
    }

    /// The ID of the agent that serves one Durable Streams session on an ephemeral or
    /// `phantomAgent` route, so every request of the session reaches one agent.
    pub fn durable_stream_session(
        environment_id: EnvironmentId,
        type_id: TypeId,
        route_scope: &str,
        session_id: &str,
    ) -> Self {
        Self::typed(
            type_id,
            HashInput::new(DURABLE_STREAM_SESSION_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(route_scope.as_bytes())
                .variable(session_id.as_bytes()),
        )
    }

    /// The ID of an imported 1.6 phantom agent. `stored_agent_name` is the full
    /// stored 1.6 name, `Type(params)[uuid]`, exactly as stored and never
    /// canonicalized, so the import and the bridge-snapshot host function agree.
    pub fn imported_phantom(
        environment_id: EnvironmentId,
        type_id: TypeId,
        stored_agent_name: &str,
    ) -> Self {
        Self::typed(
            type_id,
            HashInput::new(IMPORTED_PHANTOM_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(stored_agent_name.as_bytes()),
        )
    }

    /// A fresh ID for a create without a key, or a fork other than a Durable Streams fork.
    pub fn random(type_id: TypeId) -> Self {
        Self::from_parts(type_id.get(), rand::random::<u128>() & H_MASK)
    }

    /// The ID that the host allocates inside an agent for a fork or a create without a
    /// key. `call_key` is the idempotency key of the durable host call, derived from its
    /// position in the caller's oplog, so a replay of the call allocates the same ID and
    /// a repeated call reaches the agent the first attempt made. No key is part of the
    /// hash, so no upsert reaches the agent. `type_tag` is the source's tag for a fork
    /// and the target type's tag for a create; the hash includes it.
    pub fn allocated(
        environment_id: EnvironmentId,
        type_tag: u32,
        caller: AgentId,
        caller_fingerprint: AgentFingerprint,
        call_key: &IdempotencyKey,
    ) -> Self {
        Self::from_parts(
            type_tag,
            HashInput::new(ALLOCATED_DOMAIN)
                .environment(environment_id)
                .fixed(&type_tag.to_be_bytes())
                .fixed(caller.as_uuid().as_bytes())
                .fixed(caller_fingerprint.0.as_bytes())
                .variable(call_key.value.as_bytes())
                .h(),
        )
    }

    /// The ID of the owner of a native tool called on a component instead of on an
    /// agent. It has the reserved tag. `owner_key` is the invocation's owner key
    /// ([`crate::model::InvocationRequestKey::owner_key`]), which binds the caller scope,
    /// the operation and the idempotency key; a request without a caller scope has an owner
    /// key of its own. The first-class tools work replaces these owners.
    pub fn external_tool_owner(
        environment_id: EnvironmentId,
        component_id: ComponentId,
        owner_key: &IdempotencyKey,
    ) -> Self {
        Self::from_parts(
            RESERVED_TYPE_TAG,
            HashInput::new(EXTERNAL_TOOL_OWNER_DOMAIN)
                .environment(environment_id)
                .fixed(component_id.0.as_bytes())
                .variable(owner_key.value.as_bytes())
                .h(),
        )
    }

    /// A fresh ID with the reserved tag, for an oplog processor.
    pub fn random_reserved() -> Self {
        Self::from_parts(RESERVED_TYPE_TAG, rand::random::<u128>() & H_MASK)
    }

    fn typed(type_id: TypeId, input: HashInput) -> Self {
        Self::from_parts(type_id.get(), input.h())
    }
}

struct HashInput(Sha256);

impl HashInput {
    fn new(domain: &str) -> Self {
        Self(Sha256::new()).variable(domain.as_bytes())
    }

    fn fixed(mut self, bytes: &[u8]) -> Self {
        self.0.update(bytes);
        self
    }

    fn variable(mut self, bytes: &[u8]) -> Self {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
        self
    }

    fn environment(self, environment_id: EnvironmentId) -> Self {
        self.fixed(environment_id.0.as_bytes())
    }

    fn type_id(self, type_id: TypeId) -> Self {
        self.fixed(&type_id.get().to_be_bytes())
    }

    fn h(self) -> u128 {
        let digest = self.0.finalize();
        let mut first = [0u8; 16];
        first.copy_from_slice(&digest[..16]);
        u128::from_be_bytes(first) >> (128 - crate::base_model::agent_id::H_BITS)
    }
}

#[cfg(test)]
mod tests;
