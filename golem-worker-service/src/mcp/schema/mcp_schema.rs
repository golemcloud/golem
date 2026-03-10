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

use golem_common::base_model::agent::{
    ComponentModelElementSchema, ElementSchema, NamedElementSchema,
};
use golem_wasm::analysis::AnalysedType;
use serde_json::{Map, Value, json};

#[derive(Default)]
pub struct McpSchema {
    pub properties: Map<FieldName, JsonTypeDescription>,
    pub required: Vec<FieldName>,
}

impl From<McpSchema> for rmcp::model::JsonObject {
    fn from(value: McpSchema) -> Self {
        let json_value = json!({
            "type": "object",
            "properties": value.properties,
            "required": value.required,
        });

        rmcp::model::object(json_value)
    }
}

impl McpSchema {
    pub fn prepend_schema(&mut self, mut new_schema: McpSchema) {
        new_schema
            .properties
            .extend(std::mem::take(&mut self.properties));

        new_schema
            .required
            .extend(std::mem::take(&mut self.required));

        *self = new_schema;
    }

    pub fn from_named_element_schemas(schemas: &[NamedElementSchema]) -> McpSchema {
        let mut properties: Map<String, JsonTypeDescription> = Map::new();
        let mut required = Vec::new();

        for s in schemas {
            let schema = match &s.schema {
                ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
                    if !matches!(element_type, AnalysedType::Option(_)) {
                        required.push(s.name.clone());
                    }
                    analysed_type_to_json_schema(element_type)
                }
                ElementSchema::UnstructuredText(descriptor) => {
                    required.push(s.name.to_string());

                    let language_code_description = match &descriptor.restrictions {
                        Some(types) if !types.is_empty() => {
                            let codes: Vec<&str> =
                                types.iter().map(|t| t.language_code.as_str()).collect();
                            format!("Language code. Must be one of: {}", codes.join(", "))
                        }
                        _ => "Language code".to_string(),
                    };
                    json!({
                        "type": "object",
                        "properties": {
                            "data": {"type": "string", "description": "Text content"},
                            "languageCode": {"type": "string", "description": language_code_description}
                        },
                        "required": ["data"]
                    })
                }
                ElementSchema::UnstructuredBinary(descriptor) => {
                    required.push(s.name.clone());
                    let mime_type_description = match &descriptor.restrictions {
                        Some(types) if !types.is_empty() => {
                            let mimes: Vec<&str> =
                                types.iter().map(|t| t.mime_type.as_str()).collect();
                            format!("MIME type. Must be one of: {}", mimes.join(", "))
                        }
                        _ => "MIME type".to_string(),
                    };

                    json!({
                        "type": "object",
                        "properties": {
                            "data": {"type": "string", "description": "Base64-encoded binary data"},
                            "mimeType": {"type": "string", "description": mime_type_description}
                        },
                        "required": ["data", "mimeType"]
                    })
                }
            };
            properties.insert(s.name.clone(), schema);
        }

        McpSchema {
            properties,
            required,
        }
    }

    pub fn from_multimodal_element_schemas(schemas: &[NamedElementSchema]) -> McpSchema {
        let one_of: Vec<Value> = schemas
            .iter()
            .map(|s| {
                let value_schema = element_schema_to_json_schema(&s.schema);
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "const": s.name},
                        "value": value_schema
                    },
                    "required": ["name", "value"],
                    "additionalProperties": false
                })
            })
            .collect();

        let array_schema = json!({
            "type": "array",
            "items": {
                "oneOf": one_of
            }
        });

        let mut properties: Map<String, JsonTypeDescription> = Map::new();
        properties.insert("parts".to_string(), array_schema);

        McpSchema {
            properties,
            required: vec!["parts".to_string()],
        }
    }

    pub fn from_record_fields(fields: &[(&str, &AnalysedType)]) -> McpSchema {
        let mut properties: Map<String, JsonTypeDescription> = Map::new();
        let mut required = Vec::new();

        for (name, typ) in fields {
            properties.insert(name.to_string(), analysed_type_to_json_schema(typ));
            if !matches!(typ, AnalysedType::Option(_)) {
                required.push(name.to_string());
            }
        }

        McpSchema {
            properties,
            required,
        }
    }
}

pub type JsonTypeDescription = Value;
pub type FieldName = String;

fn element_schema_to_json_schema(schema: &ElementSchema) -> JsonTypeDescription {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            analysed_type_to_json_schema(element_type)
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let language_code_description = match &descriptor.restrictions {
                Some(types) if !types.is_empty() => {
                    let codes: Vec<&str> = types.iter().map(|t| t.language_code.as_str()).collect();
                    format!("Language code. Must be one of: {}", codes.join(", "))
                }
                _ => "Language code".to_string(),
            };
            json!({
                "type": "object",
                "properties": {
                    "data": {"type": "string", "description": "Text content"},
                    "languageCode": {"type": "string", "description": language_code_description}
                },
                "required": ["data"]
            })
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let mime_type_description = match &descriptor.restrictions {
                Some(types) if !types.is_empty() => {
                    let mimes: Vec<&str> = types.iter().map(|t| t.mime_type.as_str()).collect();
                    format!("MIME type. Must be one of: {}", mimes.join(", "))
                }
                _ => "MIME type".to_string(),
            };
            json!({
                "type": "object",
                "properties": {
                    "data": {"type": "string", "description": "Base64-encoded binary data"},
                    "mimeType": {"type": "string", "description": mime_type_description}
                },
                "required": ["data", "mimeType"]
            })
        }
    }
}

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

            let schema = McpSchema::from_record_fields(&fields);

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
