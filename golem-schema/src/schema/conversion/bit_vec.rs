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
impl crate::schema::wit::direct::WireSchema for ::bit_vec::BitVec {
    fn wire_type_id() -> String {
        <Self as IntoSchema>::type_id().0
    }

    fn append_schema(builder: &mut crate::schema::wit::direct::WireSchemaBuilder) -> i32 {
        <Vec<bool> as crate::schema::wit::direct::WireSchema>::append_schema(builder)
    }
}

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::IntoWire for ::bit_vec::BitVec {
    const NEEDS_PREPARATION: bool = false;

    fn write_wire(
        &self,
        writer: &mut crate::schema::wit::direct::WireWriter,
    ) -> Result<i32, crate::schema::wit::direct::WireError> {
        use crate::schema::wit::wire::SchemaValueNode;
        let elements = self
            .iter()
            .map(|value| writer.push(SchemaValueNode::BoolValue(value)))
            .collect();
        Ok(writer.push(SchemaValueNode::ListValue(elements)))
    }
}

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::FromWire for ::bit_vec::BitVec {
    fn read_wire(
        reader: &mut crate::schema::wit::direct::WireReader,
        index: i32,
    ) -> Result<Self, crate::schema::wit::direct::WireError> {
        <Vec<bool> as crate::schema::wit::direct::FromWire>::read_wire(reader, index)
            .map(|values| values.into_iter().collect())
    }
}

impl IntoSchema for ::bit_vec::BitVec {
    fn type_id() -> TypeId {
        TypeId::new("bit_vec.BitVec")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::list(SchemaType::bool())
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::List {
            elements: self.iter().map(SchemaValue::Bool).collect(),
        }
    }
}

impl FromSchema for ::bit_vec::BitVec {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::List { elements } => {
                let mut bv = ::bit_vec::BitVec::with_capacity(elements.len());
                for e in elements {
                    match e {
                        SchemaValue::Bool(b) => bv.push(*b),
                        other => {
                            return Err(FromSchemaError::shape_mismatch(
                                "bool",
                                value_kind(other),
                                "BitVec",
                            ));
                        }
                    }
                }
                Ok(bv)
            }
            other => Err(FromSchemaError::shape_mismatch(
                "list",
                value_kind(other),
                "BitVec",
            )),
        }
    }
}
