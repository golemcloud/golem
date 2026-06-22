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

use crate::schema::conversion::{
    FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, value_kind,
};
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;

macro_rules! impl_schema_for_uuid_newtype {
    ($ty:ty, $type_id:literal, $name:literal) => {
        impl IntoSchema for $ty {
            fn type_id() -> TypeId {
                TypeId::new($type_id)
            }

            fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
                let id = <Self as IntoSchema>::type_id();
                if builder.is_registered(&id) {
                    return SchemaType::ref_to(id);
                }
                builder.reserve(id.clone());
                let uuid_ty = <uuid::Uuid as IntoSchema>::register_in(builder);
                let body = SchemaType::record(vec![NamedFieldType {
                    name: "uuid".to_string(),
                    body: uuid_ty,
                    metadata: MetadataEnvelope::default(),
                }]);
                builder.commit(
                    id.clone(),
                    Some($name.to_string()),
                    MetadataEnvelope::default(),
                    body,
                );
                SchemaType::ref_to(id)
            }

            fn to_value(&self) -> SchemaValue {
                SchemaValue::Record {
                    fields: vec![<uuid::Uuid as IntoSchema>::to_value(&self.0)],
                }
            }
        }

        impl FromSchema for $ty {
            fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
                match v {
                    SchemaValue::Record { fields } if fields.len() == 1 => {
                        let inner = <uuid::Uuid as FromSchema>::from_value(&fields[0])?;
                        Ok(<$ty>::from(inner))
                    }
                    other => Err(FromSchemaError::shape_mismatch(
                        "record",
                        value_kind(other),
                        $type_id,
                    )),
                }
            }
        }
    };
}

impl_schema_for_uuid_newtype!(
    crate::base_model::AgentFingerprint,
    "golem_common.base_model.AgentFingerprint",
    "agent-fingerprint"
);
impl_schema_for_uuid_newtype!(
    crate::base_model::component::ComponentId,
    "golem_common.base_model.component.ComponentId",
    "component-id"
);
impl_schema_for_uuid_newtype!(
    crate::base_model::environment::EnvironmentId,
    "golem_common.base_model.environment.EnvironmentId",
    "environment-id"
);
impl_schema_for_uuid_newtype!(
    crate::base_model::quota::ResourceDefinitionId,
    "golem_common.base_model.quota.ResourceDefinitionId",
    "resource-definition-id"
);

impl IntoSchema for crate::base_model::component::ComponentRevision {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.base_model.component.ComponentRevision")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = <Self as IntoSchema>::type_id();
        if builder.is_registered(&id) {
            return SchemaType::ref_to(id);
        }
        builder.reserve(id.clone());
        let body = SchemaType::tuple(vec![SchemaType::u64()]);
        builder.commit(
            id.clone(),
            Some("component-revision".to_string()),
            MetadataEnvelope::default(),
            body,
        );
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Tuple {
            elements: vec![SchemaValue::U64(self.get())],
        }
    }
}

impl FromSchema for crate::base_model::component::ComponentRevision {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Tuple { elements } if elements.len() == 1 => match &elements[0] {
                SchemaValue::U64(x) => Ok(crate::base_model::component::ComponentRevision(*x)),
                other => Err(FromSchemaError::shape_mismatch(
                    "u64",
                    value_kind(other),
                    "ComponentRevision",
                )),
            },
            other => Err(FromSchemaError::shape_mismatch(
                "tuple",
                value_kind(other),
                "ComponentRevision",
            )),
        }
    }
}
