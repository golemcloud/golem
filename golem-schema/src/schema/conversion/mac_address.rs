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
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;

impl IntoSchema for ::mac_address::MacAddress {
    fn type_id() -> TypeId {
        TypeId::new("mac_address.MacAddress")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = <Self as IntoSchema>::type_id();
        if builder.is_registered(&id) {
            return SchemaType::ref_to(id);
        }
        builder.reserve(id.clone());
        let body = SchemaType::record(vec![NamedFieldType {
            name: "bytes".to_string(),
            body: SchemaType::fixed_list(SchemaType::u8(), 6),
            metadata: MetadataEnvelope::default(),
        }]);
        builder.commit(
            id.clone(),
            Some("MacAddress".to_string()),
            MetadataEnvelope::default(),
            body,
        );
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![SchemaValue::FixedList {
                elements: self.bytes().into_iter().map(SchemaValue::U8).collect(),
            }],
        }
    }
}

impl FromSchema for ::mac_address::MacAddress {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Record { fields } if fields.len() == 1 => {
                let elements = match &fields[0] {
                    SchemaValue::FixedList { elements } | SchemaValue::List { elements } => {
                        elements
                    }
                    other => {
                        return Err(FromSchemaError::shape_mismatch(
                            "fixed-list",
                            value_kind(other),
                            "MacAddress.bytes",
                        ));
                    }
                };
                if elements.len() != 6 {
                    return Err(FromSchemaError::custom("MacAddress must be 6 bytes"));
                }
                let mut bytes = [0u8; 6];
                for (idx, element) in elements.iter().enumerate() {
                    match element {
                        SchemaValue::U8(value) => bytes[idx] = *value,
                        other => {
                            return Err(FromSchemaError::shape_mismatch(
                                "u8",
                                value_kind(other),
                                "MacAddress.bytes",
                            ));
                        }
                    }
                }
                Ok(::mac_address::MacAddress::new(bytes))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "record",
                value_kind(other),
                "MacAddress",
            )),
        }
    }
}
