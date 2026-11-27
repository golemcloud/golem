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
use crate::schema::conversion::{FromSchemaError, IntoSchema, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use crate::schema::tool::DiscoveredTool;

impl IntoSchema for super::PublicRpcResponse {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.HostResponseGolemRpcInvokeAndAwait")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = Self::type_id();
        if !builder.is_registered(&id) {
            builder.reserve(id.clone());
            let body = SchemaType::record_from_fields([(
                "result",
                <Result<SchemaValue, super::SerializableRpcError>>::register_in(builder),
            )]);
            builder.commit(
                id.clone(),
                Some("HostResponseGolemRpcInvokeAndAwait".into()),
                Default::default(),
                body,
            );
        }
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![self.result.to_value()],
        }
    }
}

impl IntoSchema for super::PublicToolOperationTerminal {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.types.SerializableToolOperationTerminal")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = Self::type_id();
        if !builder.is_registered(&id) {
            builder.reserve(id.clone());
            let body = SchemaType::record_from_fields([
                (
                    "body_execution",
                    super::SerializableEntityBodyExecution::register_in(builder),
                ),
                (
                    "result",
                    <Result<
                        super::SerializableToolStructuredResult,
                        super::SerializableToolRpcError,
                    >>::register_in(builder),
                ),
                (
                    "admission_refusal",
                    <Option<super::SerializableToolAdmissionRefusal>>::register_in(builder),
                ),
            ]);
            builder.commit(
                id.clone(),
                Some("SerializableToolOperationTerminal".into()),
                Default::default(),
                body,
            );
        }
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![
                self.body_execution.to_value(),
                self.result.to_value(),
                self.admission_refusal.to_value(),
            ],
        }
    }
}

impl From<Result<SchemaValue, super::SerializableRpcError>> for super::RecordedRpcResult {
    fn from(value: Result<SchemaValue, super::SerializableRpcError>) -> Self {
        Self::Inline(value)
    }
}

impl super::RecordedRpcResult {
    pub fn into_inline(self) -> Result<Result<SchemaValue, super::SerializableRpcError>, String> {
        match self {
            Self::Inline(value) => Ok(value),
            Self::Remote(_) => Err("scalar RPC response refers to a stream journal".to_string()),
        }
    }
}

impl From<Result<super::SerializableToolStructuredResult, super::SerializableToolRpcError>>
    for super::RecordedToolResult
{
    fn from(
        value: Result<super::SerializableToolStructuredResult, super::SerializableToolRpcError>,
    ) -> Self {
        Self::Inline(value)
    }
}

impl super::RecordedToolResult {
    pub fn into_inline(
        self,
    ) -> Result<
        Result<super::SerializableToolStructuredResult, super::SerializableToolRpcError>,
        String,
    > {
        match self {
            Self::Inline(value) => Ok(value),
            Self::Remote { .. } => {
                Err("tool result reference requires its stream session".to_string())
            }
        }
    }
}

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
