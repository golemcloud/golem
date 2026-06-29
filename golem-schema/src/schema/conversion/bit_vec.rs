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
