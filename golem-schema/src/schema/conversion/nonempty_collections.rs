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

use super::{
    FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, type_id_with_args, value_kind,
};
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;

#[cfg(feature = "guest")]
use crate::schema::wit::{
    direct::{
        FromWire, IntoWire, WireError, WirePreflight, WireReader, WireSchema, WireSchemaBuilder,
        WireWriter, empty_metadata,
    },
    wire,
};

#[cfg(feature = "guest")]
impl<T: WireSchema> WireSchema for nonempty_collections::NEVec<T> {
    fn wire_type_id() -> String {
        format!("nonempty_collections.NEVec<{}>", T::wire_type_id())
    }

    fn contains_stream(seen: &mut std::collections::HashSet<&'static str>) -> bool {
        T::contains_stream(seen)
    }

    fn append_schema(builder: &mut WireSchemaBuilder) -> i32 {
        let (definition, fresh) = builder.reserve(Self::wire_type_id(), Some("NEVec".into()));
        if fresh {
            let items = Vec::<T>::append_schema(builder);
            let body = builder.push(wire::SchemaTypeBody::RecordType(vec![
                wire::NamedFieldType {
                    name: "items".into(),
                    body: items,
                    metadata: empty_metadata(),
                },
            ]));
            builder.commit(definition, body);
        }
        builder.reference(definition)
    }
}

#[cfg(feature = "guest")]
impl<T: IntoWire> IntoWire for nonempty_collections::NEVec<T> {
    const NEEDS_PREPARATION: bool = T::NEEDS_PREPARATION;

    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        for value in self.iter() {
            value.preflight(resources)?;
        }
        Ok(())
    }

    async fn prepare_wire(&self) -> Result<(), WireError> {
        if T::NEEDS_PREPARATION {
            for value in self.iter() {
                Box::pin(value.prepare_wire()).await?;
            }
        }
        Ok(())
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<i32, WireError> {
        let items = self
            .iter()
            .map(|value| value.write_wire(writer))
            .collect::<Result<_, _>>()?;
        let items = writer.push(wire::SchemaValueNode::ListValue(items));
        Ok(writer.push(wire::SchemaValueNode::RecordValue(vec![items])))
    }
}

#[cfg(feature = "guest")]
impl<T: FromWire> FromWire for nonempty_collections::NEVec<T> {
    fn read_wire(reader: &mut WireReader, index: i32) -> Result<Self, WireError> {
        let wire::SchemaValueNode::RecordValue(fields) = reader.take(index)? else {
            return Err(WireError::Shape("record"));
        };
        if fields.len() != 1 {
            return Err(WireError::Shape("one-field nonempty vector"));
        }
        let items = Vec::<T>::read_wire(reader, fields[0])?;
        Self::try_from_vec(items).ok_or(WireError::Shape("nonempty vector"))
    }
}

impl<T: IntoSchema> IntoSchema for nonempty_collections::NEVec<T> {
    fn type_id() -> crate::schema::metadata::TypeId {
        type_id_with_args("nonempty_collections.NEVec", &[T::type_id()])
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = <Self as IntoSchema>::type_id();
        if builder.is_registered(&id) {
            return SchemaType::ref_to(id);
        }
        builder.reserve(id.clone());
        let body = SchemaType::record(vec![NamedFieldType {
            name: "items".to_string(),
            body: SchemaType::list(T::register_in(builder)),
            metadata: MetadataEnvelope::default(),
        }]);
        builder.commit(
            id.clone(),
            Some("NEVec".to_string()),
            MetadataEnvelope::default(),
            body,
        );
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![SchemaValue::List {
                elements: self.iter().map(IntoSchema::to_value).collect(),
            }],
        }
    }
}

impl<T: FromSchema> FromSchema for nonempty_collections::NEVec<T> {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Record { fields } if fields.len() == 1 => match &fields[0] {
                SchemaValue::List { elements } => {
                    let values = elements
                        .iter()
                        .map(T::from_value)
                        .collect::<Result<_, _>>()?;
                    nonempty_collections::NEVec::try_from_vec(values)
                        .ok_or_else(|| FromSchemaError::custom("expected non-empty vector"))
                }
                other => Err(FromSchemaError::shape_mismatch(
                    "list",
                    value_kind(other),
                    "NEVec.items",
                )),
            },
            other => Err(FromSchemaError::shape_mismatch(
                "record",
                value_kind(other),
                "NEVec",
            )),
        }
    }
}
