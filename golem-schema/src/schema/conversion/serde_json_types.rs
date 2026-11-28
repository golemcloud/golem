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
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::WireSchema for ::serde_json::Value {
    fn wire_type_id() -> String {
        <Self as IntoSchema>::type_id().0
    }

    fn append_schema(builder: &mut crate::schema::wit::direct::WireSchemaBuilder) -> i32 {
        builder.push(crate::schema::wit::wire::SchemaTypeBody::JsonType)
    }
}

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::IntoWire for ::serde_json::Value {
    const NEEDS_PREPARATION: bool = false;

    fn write_wire(
        &self,
        writer: &mut crate::schema::wit::direct::WireWriter,
    ) -> Result<i32, crate::schema::wit::direct::WireError> {
        Ok(
            writer.push(crate::schema::wit::wire::SchemaValueNode::JsonValue(
                self.to_string(),
            )),
        )
    }
}

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::FromWire for ::serde_json::Value {
    fn read_wire(
        reader: &mut crate::schema::wit::direct::WireReader,
        index: i32,
    ) -> Result<Self, crate::schema::wit::direct::WireError> {
        let crate::schema::wit::wire::SchemaValueNode::JsonValue(value) = reader.take(index)?
        else {
            return Err(crate::schema::wit::direct::WireError::Shape("json"));
        };
        ::serde_json::from_str(&value)
            .map_err(|_| crate::schema::wit::direct::WireError::Shape("valid JSON"))
    }
}

impl IntoSchema for ::serde_json::Value {
    fn type_id() -> TypeId {
        TypeId::new("serde_json.Value")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::json()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::Json(self.to_string())
    }
}

impl FromSchema for ::serde_json::Value {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Json(s) => ::serde_json::from_str(s)
                .map_err(|e| FromSchemaError::custom(format!("invalid JSON: {e}"))),
            other => Err(FromSchemaError::shape_mismatch(
                "json",
                value_kind(other),
                "serde_json::Value",
            )),
        }
    }
}
