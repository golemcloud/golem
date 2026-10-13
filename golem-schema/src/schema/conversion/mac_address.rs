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

#[cfg(feature = "guest")]
use crate::schema::wit::{
    direct::{
        FromWire, IntoWire, WireError, WireReader, WireSchema, WireSchemaBuilder, WireWriter,
        empty_metadata,
    },
    wire,
};

#[cfg(feature = "guest")]
impl WireSchema for ::mac_address::MacAddress {
    fn wire_type_id() -> String {
        <Self as IntoSchema>::type_id().0
    }

    fn append_schema(builder: &mut WireSchemaBuilder) -> i32 {
        let (definition, fresh) = builder.reserve(Self::wire_type_id(), Some("MacAddress".into()));
        if fresh {
            let element = u8::append_schema(builder);
            let bytes = builder.push(wire::SchemaTypeBody::FixedListType(wire::FixedListSpec {
                element,
                length: 6,
            }));
            let body = builder.push(wire::SchemaTypeBody::RecordType(vec![
                wire::NamedFieldType {
                    name: "bytes".into(),
                    body: bytes,
                    metadata: empty_metadata(),
                },
            ]));
            builder.commit(definition, body);
        }
        builder.reference(definition)
    }
}

#[cfg(feature = "guest")]
impl IntoWire for ::mac_address::MacAddress {
    const NEEDS_PREPARATION: bool = false;

    fn write_wire(&self, writer: &mut WireWriter) -> Result<i32, WireError> {
        let bytes = self
            .bytes()
            .into_iter()
            .map(|value| writer.push(wire::SchemaValueNode::U8Value(value)))
            .collect();
        let bytes = writer.push(wire::SchemaValueNode::FixedListValue(bytes));
        Ok(writer.push(wire::SchemaValueNode::RecordValue(vec![bytes])))
    }
}

#[cfg(feature = "guest")]
impl FromWire for ::mac_address::MacAddress {
    fn read_wire(reader: &mut WireReader, index: i32) -> Result<Self, WireError> {
        let wire::SchemaValueNode::RecordValue(fields) = reader.take(index)? else {
            return Err(WireError::Shape("record"));
        };
        if fields.len() != 1 {
            return Err(WireError::Shape("one-field MAC address"));
        }
        let bytes = match reader.take(fields[0])? {
            wire::SchemaValueNode::FixedListValue(elements)
            | wire::SchemaValueNode::ListValue(elements) => elements,
            _ => return Err(WireError::Shape("fixed-list")),
        };
        if bytes.len() != 6 {
            return Err(WireError::Shape("six-byte MAC address"));
        }
        let mut result = [0; 6];
        for (value, index) in result.iter_mut().zip(bytes) {
            *value = u8::read_wire(reader, index)?;
        }
        Ok(Self::new(result))
    }
}

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
