// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::mcp::schema::McpSchema;
use golem_common::base_model::agent::{
    ComponentModelElementSchema, ElementSchema, NamedElementSchema,
};
use golem_wasm::analysis::AnalysedType;
use serde_json::{Map, Value, json};

// A better internal representation of McpSchema for object types
#[derive(Default)]
pub struct McpSchemaInternal {
    pub properties: Map<FieldName, JsonTypeDescription>,
    pub required: Vec<FieldName>,
}

impl From<McpSchemaInternal> for McpSchema {
    fn from(value: McpSchemaInternal) -> Self {
        let json_value = json!({
            "type": "object",
            "properties": value.properties,
            "required": value.required,
        });

        rmcp::model::object(json_value)
    }
}

impl McpSchemaInternal {
    pub fn prepend_schema(&mut self, mut new_schema: McpSchemaInternal) {
        new_schema
            .properties
            .extend(std::mem::take(&mut self.properties));

        new_schema
            .required
            .extend(std::mem::take(&mut self.required));

        *self = new_schema;
    }

    pub fn from_named_element_schemas(schemas: &[NamedElementSchema]) -> McpSchemaInternal {
        let named_types: Vec<(&str, &AnalysedType)> = schemas
            .iter()
            .map(|s| match &s.schema {
                ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
                    (s.name.as_str(), element_type)
                }
                _ => todo!("Unsupported element schema type in MCP schema mapping"),
            })
            .collect();

        Self::from_record_fields(&named_types)
    }

    pub fn from_record_fields(fields: &[(&str, &AnalysedType)]) -> McpSchemaInternal {
        let mut properties: Map<String, JsonTypeDescription> = Map::new();
        let mut required = Vec::new();

        for (name, typ) in fields {
            properties.insert(name.to_string(), analysed_type_to_json_schema(typ));
            if !matches!(typ, AnalysedType::Option(_)) {
                required.push(name.to_string());
            }
        }

        McpSchemaInternal {
            properties,
            required,
        }
    }
}

pub type JsonTypeDescription = Value;
pub type FieldName = String;

// Based on https://modelcontextprotocol.io/specification/2025-11-25/server/tools and
// https://json-schema.org/draft/2020-12/json-schema-core (Example: oneOf)
// while ensuring how golem-wasm treats JSON values
fn analysed_type_to_json_schema(analysed_type: &AnalysedType) -> JsonTypeDescription {
    match analysed_type {
        AnalysedType::Bool(_) => json!({"type": "boolean"}),
        AnalysedType::Str(_) => json!({"type": "string"}),
        AnalysedType::Chr(_) => json!({"type": "integer"}),
        AnalysedType::U8(_)
        | AnalysedType::U16(_)
        | AnalysedType::U32(_)
        | AnalysedType::U64(_)
        | AnalysedType::S8(_)
        | AnalysedType::S16(_)
        | AnalysedType::S32(_)
        | AnalysedType::S64(_) => json!({"type": "integer"}),
        AnalysedType::F32(_) | AnalysedType::F64(_) => json!({"type": "number"}),

        AnalysedType::List(type_list) => {
            let items = analysed_type_to_json_schema(&type_list.inner);
            json!({"type": "array", "items": items})
        }

        AnalysedType::Tuple(type_tuple) => {
            let prefix_items: Vec<Value> = type_tuple
                .items
                .iter()
                .map(analysed_type_to_json_schema)
                .collect();

            json!({
                "type": "array",
                "prefixItems": prefix_items,
                "items": false
            })
        }

        AnalysedType::Record(type_record) => {
            let fields: Vec<(&str, &AnalysedType)> = type_record
                .fields
                .iter()
                .map(|f| (f.name.as_str(), &f.typ))
                .collect();

            let schema = McpSchemaInternal::from_record_fields(&fields);

            json!({
                "type": "object",
                "properties": schema.properties,
                "required": schema.required
            })
        }

        AnalysedType::Option(type_option) => {
            let inner = analysed_type_to_json_schema(&type_option.inner);
            json!({
                "oneOf": [
                    inner,
                    {"type": "null"}
                ]
            })
        }

        AnalysedType::Enum(type_enum) => {
            json!({"type": "string", "enum": type_enum.cases})
        }

        AnalysedType::Flags(type_flags) => {
            json!({
                "type": "array",
                "items": {"type": "string", "enum": type_flags.names},
                "uniqueItems": true
            })
        }

        AnalysedType::Variant(type_variant) => {
            let one_of: Vec<Value> = type_variant
                .cases
                .iter()
                .map(|case| {
                    let value_schema = match &case.typ {
                        Some(payload_type) => analysed_type_to_json_schema(payload_type),
                        None => json!({"type": "null"}),
                    };
                    json!({
                        "type": "object",
                        "properties": {
                            case.name.clone(): value_schema,
                        },
                        "required": [case.name],
                        "additionalProperties": false
                    })
                })
                .collect();
            json!({"oneOf": one_of})
        }

        AnalysedType::Result(type_result) => {
            let ok_schema = match &type_result.ok {
                Some(ok_type) => analysed_type_to_json_schema(ok_type),
                None => json!({"type": "null"}),
            };
            let err_schema = match &type_result.err {
                Some(err_type) => analysed_type_to_json_schema(err_type),
                None => json!({"type": "null"}),
            };
            json!({
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {"ok": ok_schema},
                        "required": ["ok"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {"err": err_schema},
                        "required": ["err"],
                        "additionalProperties": false
                    }
                ]
            })
        }

        AnalysedType::Handle(_) => {
            json!({"type": "string"})
        }
    }
}
