// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::model::oplog::public_oplog_entry::BinaryCodec;
use crate::model::oplog::PayloadId;
use desert_rust::{
    BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use std::fmt::Debug;
use golem_wasm::ValueAndType;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OplogPayload<T: BinaryCodec + Debug + Clone + PartialEq> {
    Inline(T),
    SerializedInline(Vec<u8>),
    External {
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    },
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> BinarySerializer for OplogPayload<T> {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match self {
            OplogPayload::Inline(value) => {
                context.write_u8(0);
                let bytes = desert_rust::serialize_to_byte_vec(value)?;
                bytes.serialize(context)
            }
            OplogPayload::SerializedInline(bytes) => {
                context.write_u8(0);
                bytes.serialize(context)
            }
            OplogPayload::External {
                payload_id,
                md5_hash,
            } => {
                context.write_u8(1);
                payload_id.serialize(context)?;
                md5_hash.serialize(context)
            }
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> BinaryDeserializer for OplogPayload<T> {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        match tag {
            0 => {
                let bytes = Vec::<u8>::deserialize(context)?;
                Ok(Self::SerializedInline(bytes))
            }
            1 => {
                let payload_id = PayloadId::deserialize(context)?;
                let md5_hash = Vec::<u8>::deserialize(context)?;
                Ok(Self::External {
                    payload_id,
                    md5_hash,
                })
            }
            other => Err(desert_rust::Error::DeserializationFailure(format!(
                "Invalid tag for OplogPayload: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
pub enum HostRequest {
    Custom(ValueAndType)
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
pub enum HostResponse {
    Custom(ValueAndType)
}