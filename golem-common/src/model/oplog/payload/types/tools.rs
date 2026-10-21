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

use serde::Deserialize;

use super::{
    SerializableDiscoveredTools, SerializableToolPositionalRequest, SerializableToolResultValue,
};
use crate::schema::TypedSchemaValue;
use crate::schema::conversion::{FromSchemaError, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use crate::schema::tool::DiscoveredTool;

impl crate::schema::conversion::IntoSchema for SerializableDiscoveredTools {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableDiscoveredTools")
    }

    fn register_in(_builder: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }

    fn to_value(&self) -> SchemaValue {
        stacker::maybe_grow(2 << 20, 64 << 20, || {
            SchemaValue::String(
                serde_json::to_string(&self.0).expect("discovered tools are JSON-serializable"),
            )
        })
    }
}

impl crate::schema::conversion::FromSchema for SerializableDiscoveredTools {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::String(json) => stacker::maybe_grow(2 << 20, 64 << 20, || {
                // These are already validated internal definitions, not upstream MCP input.
                let mut decoder = serde_json::Deserializer::from_str(json);
                decoder.disable_recursion_limit();
                let tools = <Vec<DiscoveredTool> as Deserialize>::deserialize(&mut decoder)
                    .map_err(|error| FromSchemaError::custom(error.to_string()))?;
                decoder
                    .end()
                    .map_err(|error| FromSchemaError::custom(error.to_string()))?;
                Ok(Self(tools))
            }),
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "SerializableDiscoveredTools",
            )),
        }
    }
}

impl SerializableToolResultValue {
    pub fn from_typed(value: &TypedSchemaValue) -> Result<Self, String> {
        Ok(Self {
            value: value.clone(),
        })
    }

    pub fn into_typed(self) -> Result<TypedSchemaValue, String> {
        Ok(self.value)
    }
}

impl SerializableToolPositionalRequest {
    pub fn expected_description(self) -> &'static str {
        match self {
            Self::SetRetryPolicy => "SetRetryPolicy",
            Self::RemoveRetryPolicy => "RemoveRetryPolicy",
        }
    }
}
