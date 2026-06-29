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

use super::{FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{BinaryRestrictions, SchemaType};
use crate::schema::schema_value::{BinaryValuePayload, SchemaValue};

impl IntoSchema for ::bytes::Bytes {
    fn type_id() -> TypeId {
        TypeId::new("bytes.Bytes")
    }

    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::binary(BinaryRestrictions::default())
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Binary(BinaryValuePayload {
            bytes: self.to_vec(),
            mime_type: None,
        })
    }
}

impl FromSchema for ::bytes::Bytes {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Binary(payload) => Ok(::bytes::Bytes::from(payload.bytes.clone())),
            other => Err(FromSchemaError::shape_mismatch(
                "binary",
                value_kind(other),
                "Bytes",
            )),
        }
    }
}
