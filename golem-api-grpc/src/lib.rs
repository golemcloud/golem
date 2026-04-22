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

#[allow(clippy::large_enum_variant)]
pub mod proto {
    use crate::proto::golem::worker::UpdateMode;
    use desert_rust::{
        BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
        SerializationContext,
    };
    use golem_wasm::analysis::{AnalysedType, analysed_type};
    use golem_wasm::{FromValue, IntoValue, Value};

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

    impl IntoValue for UpdateMode {
        fn into_value(self) -> Value {
            match self {
                UpdateMode::Automatic => Value::Enum(0),
                UpdateMode::Manual => Value::Enum(1),
            }
        }

        fn get_type() -> AnalysedType {
            analysed_type::r#enum(&["automatic", "snapshot-based"])
        }
    }

    impl FromValue for UpdateMode {
        fn from_value(value: Value) -> Result<Self, String> {
            match value {
                Value::Enum(0) => Ok(UpdateMode::Automatic),
                Value::Enum(1) => Ok(UpdateMode::Manual),
                _ => Err(format!("Unexpected value for UpdateMode: {value:?}")),
            }
        }
    }
}
