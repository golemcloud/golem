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
