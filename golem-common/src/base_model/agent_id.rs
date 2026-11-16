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

//! The agent ID: an RFC 9562 version 8 UUID that carries the agent's type tag.
//!
//! Layout of the 122 free bits, in UUID byte order:
//!
//! - the 32-bit type tag, big-endian, in the leading bits of `custom_a` (the first
//!   8 hex characters of the UUID text);
//! - H, 90 bits, in the remaining 16 bits of `custom_a`, the 12 bits of `custom_b`
//!   and the 62 bits of `custom_c`, most significant bit first.
//!
//! Golem issues every agent ID; the derivations of H are in `model::agent::agent_uuid`.
//! Any other UUID is an ID that no agent has.

use uuid::{Builder, Uuid};

use crate::base_model::type_id::TypeId;

pub(crate) const H_BITS: u32 = 90;
pub(crate) const H_MASK: u128 = (1 << H_BITS) - 1;

/// The type tag that no agent type or tool gets. External-tool owners and oplog
/// processors, which have no agent type, use it.
pub const RESERVED_TYPE_TAG: u32 = 0;

crate::newtype_uuid!(issued AgentId, golem_api_grpc::proto::golem::worker::AgentId);

impl AgentId {
    /// The agent ID with this type tag and these 90 bits of H. Golem issues IDs through the
    /// derivations in `model::agent::agent_uuid`, so this is only for tests.
    pub fn from_parts(type_tag: u32, h: u128) -> Self {
        assert_eq!(h & !H_MASK, 0, "H has more than {H_BITS} bits");
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

    /// Whether the UUID has the layout of an issued agent ID: RFC 9562 version 8 with the
    /// RFC variant. Golem never issues any other UUID as an agent ID.
    pub fn is_issued(&self) -> bool {
        self.0.get_version_num() == 8 && self.0.get_variant() == uuid::Variant::RFC4122
    }

    /// The leading 32 bits. Tag 0 is reserved and belongs to no type.
    pub fn type_tag(&self) -> u32 {
        let b = self.0.as_bytes();
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }

    /// The type ID in the tag, or `None` for the reserved tag.
    pub fn type_id(&self) -> Option<TypeId> {
        TypeId::try_from(self.type_tag()).ok()
    }

    /// A non-negative shard routing hash: the top 63 bits of H, never the type tag,
    /// so the agents of one type spread over every shard.
    pub fn routing_hash(&self) -> i64 {
        (self.h() >> (H_BITS - 63)) as i64
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    pub(crate) fn h(&self) -> u128 {
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

impl AsRef<AgentId> for &AgentId {
    fn as_ref(&self) -> &AgentId {
        self
    }
}
