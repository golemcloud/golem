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

impl IntoSchema for golem_api_grpc::proto::golem::worker::UpdateMode {
    fn type_id() -> TypeId {
        TypeId::new("golem_api_grpc.proto.golem.worker.UpdateMode")
    }

    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::r#enum(vec!["automatic".to_string(), "snapshot-based".to_string()])
    }

    fn to_value(&self) -> SchemaValue {
        use golem_api_grpc::proto::golem::worker::UpdateMode;
        match self {
            UpdateMode::Automatic => SchemaValue::Enum { case: 0 },
            UpdateMode::Manual => SchemaValue::Enum { case: 1 },
        }
    }
}

impl FromSchema for golem_api_grpc::proto::golem::worker::UpdateMode {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        use golem_api_grpc::proto::golem::worker::UpdateMode;
        match v {
            SchemaValue::Enum { case: 0 } => Ok(UpdateMode::Automatic),
            SchemaValue::Enum { case: 1 } => Ok(UpdateMode::Manual),
            SchemaValue::Enum { case } => {
                Err(FromSchemaError::out_of_range(*case, 2, "UpdateMode"))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "enum",
                value_kind(other),
                "UpdateMode",
            )),
        }
    }
}
