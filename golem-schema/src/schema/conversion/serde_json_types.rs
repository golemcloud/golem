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

impl IntoSchema for ::serde_json::Value {
    fn type_id() -> TypeId {
        TypeId::new("serde_json.Value")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.to_string())
    }
}

impl FromSchema for ::serde_json::Value {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => ::serde_json::from_str(s)
                .map_err(|e| FromSchemaError::custom(format!("invalid JSON: {e}"))),
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "serde_json::Value",
            )),
        }
    }
}
