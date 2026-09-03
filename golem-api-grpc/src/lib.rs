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

#[cfg(test)]
test_r::enable!();

pub mod invocation_session_protocol;

#[allow(clippy::large_enum_variant)]
pub mod proto {
    use crate::proto::golem::worker::UpdateMode;
    use desert_rust::{
        BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
        SerializationContext,
    };

    use uuid::Uuid;

    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    impl From<Uuid> for golem::common::Uuid {
        fn from(value: Uuid) -> Self {
            let (high_bits, low_bits) = value.as_u64_pair();
            golem::common::Uuid {
                high_bits,
                low_bits,
            }
        }
    }

    impl From<golem::common::Uuid> for Uuid {
        fn from(value: golem::common::Uuid) -> Self {
            let high_bits = value.high_bits;
            let low_bits = value.low_bits;
            Uuid::from_u64_pair(high_bits, low_bits)
        }
    }

    impl BinarySerializer for UpdateMode {
        fn serialize<Output: BinaryOutput>(
            &self,
            context: &mut SerializationContext<Output>,
        ) -> desert_rust::Result<()> {
            match self {
                UpdateMode::Automatic => 0u8.serialize(context),
                UpdateMode::Manual => 1u8.serialize(context),
            }
        }
    }

    impl BinaryDeserializer for UpdateMode {
        fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
            match u8::deserialize(context)? {
                0u8 => Ok(UpdateMode::Automatic),
                1u8 => Ok(UpdateMode::Manual),
                other => Err(desert_rust::Error::InvalidConstructorId {
                    constructor_id: other as u32,
                    type_name: "UpdateMode".to_string(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::golem::shardmanager::ShardId;
    use crate::proto::golem::workerexecutor::v1::AssignShardsRequest;
    use prost::Message;
    use test_r::test;

    /// Encodes one `repeated golem.shardmanager.ShardId shard_ids = 1` element
    /// the way the pre-ticket-4 `AssignShardsRequest` did: field 1, wire type 2
    /// (length-delimited), carrying an encoded `ShardId` submessage.
    fn old_shard_ids_field(values: &[i64]) -> Vec<u8> {
        let mut buf = Vec::new();
        for value in values {
            let shard_id = ShardId { value: *value }.encode_to_vec();
            // tag = (field_number << 3) | wire_type = (1 << 3) | 2 = 0x0a
            buf.push(0x0a);
            prost::encoding::encode_varint(shard_id.len() as u64, &mut buf);
            buf.extend_from_slice(&shard_id);
        }
        buf
    }

    /// Field 1 is `reserved` in `AssignShardsRequest`, so an old client's bytes
    /// must be *ignored*, not rejected: the message decodes with every current
    /// field left at its default.
    #[test]
    fn old_assign_shards_bytes_decode_into_the_reserved_field_shape() {
        let bytes = old_shard_ids_field(&[0, 7, 42]);
        // Three length-delimited entries; the `0` shard encodes as an empty
        // submessage, so this is 2 + 4 + 4 bytes.
        assert_eq!(
            bytes,
            vec![0x0a, 0x00, 0x0a, 0x02, 0x08, 0x07, 0x0a, 0x02, 0x08, 0x2a]
        );

        let decoded = AssignShardsRequest::decode(bytes.as_slice())
            .expect("old-shape bytes must decode, not error");

        assert!(decoded.shard_epochs.is_empty());
        assert_eq!(decoded.expires_at, None);
        assert_eq!(decoded.number_of_shards, 0);
    }
}
