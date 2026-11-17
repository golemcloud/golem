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

//! Agent IDs: RFC 9562 version 8 UUIDs that carry the agent's type tag.
//!
//! Layout of the 122 free bits, in UUID byte order:
//!
//! - the 32-bit type tag, big-endian, in the leading bits of `custom_a` (the first
//!   8 hex characters of the UUID text);
//! - H, 90 bits, in the remaining 16 bits of `custom_a`, the 12 bits of `custom_b`
//!   and the 62 bits of `custom_c`, most significant bit first.
//!
//! A derived H is the first 90 bits of a SHA-256 digest. Each derivation hashes its
//! own domain tag first, and every variable-length input, the domain tag included,
//! is a `u64` big-endian byte length followed by its bytes. Environment IDs and agent
//! IDs are 16 raw bytes and type IDs are 4 big-endian bytes.
//!
//! The derivations, their domain tags and this layout are part of every derived
//! agent ID: changing any of them moves every such agent to another address. The
//! golden tests of this module guard them.

use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};
use uuid::{Builder, Uuid, Variant, Version};

use super::AgentKey;
use crate::base_model::IdempotencyKey;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::type_id::TypeId;

const H_BITS: u32 = 90;
const H_MASK: u128 = (1 << H_BITS) - 1;

const KEYED_DOMAIN: &str = "golem:agent-id:keyed:v1";
const IDEMPOTENT_CREATE_DOMAIN: &str = "golem:agent-id:idempotent-create:v1";
const EPHEMERAL_INVOCATION_DOMAIN: &str = "golem:agent-id:ephemeral-invocation:v1";
const DURABLE_STREAM_FORK_DOMAIN: &str = "golem:agent-id:durable-stream-fork:v1";
const DURABLE_STREAM_SESSION_DOMAIN: &str = "golem:agent-id:durable-stream-session:v1";
const IMPORTED_PHANTOM_DOMAIN: &str = "golem:agent-id:imported-phantom:v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentUuid(Uuid);

impl AgentUuid {
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
        caller_scope: &str,
        idempotency_key: &IdempotencyKey,
    ) -> Self {
        Self::typed(
            type_id,
            HashInput::new(IDEMPOTENT_CREATE_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(caller_scope.as_bytes())
                .variable(idempotency_key.value.as_bytes()),
        )
    }

    /// The ID of the instance that runs an ephemeral invocation, so a retry reaches
    /// the same instance. A caller without a caller scope never reaches this
    /// derivation: its idempotency key is ignored, and it gets [`Self::random`].
    pub fn ephemeral_invocation(
        environment_id: EnvironmentId,
        type_id: TypeId,
        caller_scope: &str,
        idempotency_key: &IdempotencyKey,
    ) -> Self {
        Self::typed(
            type_id,
            HashInput::new(EPHEMERAL_INVOCATION_DOMAIN)
                .environment(environment_id)
                .type_id(type_id)
                .variable(caller_scope.as_bytes())
                .variable(idempotency_key.value.as_bytes()),
        )
    }

    /// The ID of a Durable Streams fork. It carries the root's type tag.
    pub fn durable_stream_fork(
        environment_id: EnvironmentId,
        root: AgentUuid,
        fork_name: &str,
    ) -> Self {
        Self::from_parts(
            root.type_tag(),
            HashInput::new(DURABLE_STREAM_FORK_DOMAIN)
                .environment(environment_id)
                .fixed(root.0.as_bytes())
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

    /// The leading 32 bits. Tag 0 is reserved and belongs to no type.
    pub fn type_tag(&self) -> u32 {
        let b = self.0.as_bytes();
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }

    /// A non-negative shard routing hash: the top 63 bits of H, never the type tag,
    /// so the agents of one type spread over every shard.
    pub fn routing_hash(&self) -> i64 {
        (self.h() >> (H_BITS - 63)) as i64
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    fn typed(type_id: TypeId, input: HashInput) -> Self {
        Self::from_parts(type_id.get(), input.h())
    }

    fn from_parts(type_tag: u32, h: u128) -> Self {
        debug_assert_eq!(h & !H_MASK, 0);
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&type_tag.to_be_bytes());
        bytes[4..6].copy_from_slice(&((h >> 74) as u16).to_be_bytes());
        let custom_b = ((h >> 62) & 0xFFF) as u16;
        bytes[6] = (custom_b >> 8) as u8;
        bytes[7] = custom_b as u8;
        let custom_c = (h & ((1 << 62) - 1)) as u64;
        bytes[8..16].copy_from_slice(&custom_c.to_be_bytes());
        Self(Builder::from_custom_bytes(bytes).into_uuid())
    }

    fn h(&self) -> u128 {
        let b = self.0.as_bytes();
        let custom_a = u128::from(u16::from_be_bytes([b[4], b[5]]));
        let custom_b = (u128::from(b[6] & 0x0F) << 8) | u128::from(b[7]);
        let mut c = [0u8; 8];
        c.copy_from_slice(&b[8..16]);
        c[0] &= 0x3F;
        let custom_c = u128::from(u64::from_be_bytes(c));
        (custom_a << 74) | (custom_b << 62) | custom_c
    }
}

impl TryFrom<Uuid> for AgentUuid {
    type Error = String;

    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        if value.get_version() == Some(Version::Custom) && value.get_variant() == Variant::RFC4122 {
            Ok(Self(value))
        } else {
            Err(format!(
                "Agent ID {value} is not an RFC 9562 version 8 UUID"
            ))
        }
    }
}

impl Display for AgentUuid {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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
        u128::from_be_bytes(first) >> (128 - H_BITS)
    }
}

#[cfg(test)]
mod tests;
