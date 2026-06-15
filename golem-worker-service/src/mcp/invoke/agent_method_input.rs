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

use crate::mcp::invoke::multimodal_params_extraction::extract_multimodal_element_value;
use golem_common::base_model::agent::{
    BinaryReference, ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
    TextReference,
};
use golem_common::schema::adapters::value_to_schema_value;
use golem_common::schema::{
    BinaryValuePayload, SchemaValue, TextValuePayload, VariantValuePayload,
};
use golem_wasm::analysis::AnalysedType;
use rmcp::model::JsonObject;

pub fn get_agent_method_input(
    mcp_args: &JsonObject,
    schema: &DataSchema,
) -> Result<SchemaValue, String> {
    match schema {
        DataSchema::Tuple(named_schemas) => {
            let fields = extract_element_values(mcp_args, &named_schemas.elements)?;
            Ok(SchemaValue::Record { fields })
        }
        DataSchema::Multimodal(named_schemas) => {
            let parts_array = mcp_args
                .get("parts")
                .and_then(|v| v.as_array())
                .ok_or("Multimodal input requires a parts array field")?;

            let schema_map: std::collections::HashMap<&str, (usize, &ElementSchema)> =
                named_schemas
                    .elements
                    .iter()
                    .enumerate()
                    .map(|(case, s)| (s.name.as_str(), (case, &s.schema)))
                    .collect();

            let mut elements = Vec::new();
            for (i, part) in parts_array.iter().enumerate() {
                // Each multimodal part is a canonical variant object: a
                // single-key object `{ <element name>: <value> }`.
                let obj = part.as_object().ok_or_else(|| {
                    format!(
                        "parts[{}] must be a single-key object {{ <name>: <value> }}",
                        i
                    )
                })?;
                if obj.len() != 1 {
                    return Err(format!(
                        "parts[{}] must be a single-key object {{ <name>: <value> }}, got {} keys",
                        i,
                        obj.len()
                    ));
                }
                let (name, value_json) = obj.iter().next().expect("object has exactly one entry");

                let (case, elem_schema) = schema_map.get(name.as_str()).ok_or_else(|| {
                    format!(
                        "parts[{}]: unknown element name '{}'. Expected one of: {}",
                        i,
                        name,
                        schema_map.keys().copied().collect::<Vec<_>>().join(", ")
                    )
                })?;

                let body = extract_multimodal_element_value(name, value_json, elem_schema, i)?;

                elements.push(SchemaValue::Variant(VariantValuePayload {
                    case: *case as u32,
                    payload: Some(Box::new(body)),
                }));
            }
            Ok(SchemaValue::Record {
                fields: vec![SchemaValue::List { elements }],
            })
        }
    }
}

fn extract_element_values(
    args_map: &JsonObject,
    schemas: &[NamedElementSchema],
) -> Result<Vec<SchemaValue>, String> {
    let mut params = Vec::new();
    for schema_element in schemas {
        let element =
            extract_single_element_value(args_map, &schema_element.name, &schema_element.schema)?;
        params.push(element);
    }
    Ok(params)
}

fn extract_single_element_value(
    args_map: &JsonObject,
    name: &str,
    elem_schema: &ElementSchema,
) -> Result<SchemaValue, String> {
    let json_value = args_map.get(name);
    match elem_schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let json_value = match json_value {
                Some(value) => value.clone(),
                None => {
                    if matches!(element_type, AnalysedType::Option(_)) {
                        serde_json::Value::Null
                    } else {
                        return Err(format!("Missing parameter: {}", name));
                    }
                }
            };

            let value = crate::mcp::invoke::parse_component_model_value(&json_value, element_type)
                .map_err(|e| format!("Failed to parse parameter '{}': {}", name, e))?;

            value_to_schema_value(&value, element_type)
                .map_err(|e| format!("Failed to convert parameter '{}': {}", name, e))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let json_value = json_value.ok_or_else(|| format!("Missing parameter: {}", name))?;
            let value = crate::mcp::invoke::parse_unstructured_text(json_value, descriptor)
                .map_err(|e| format!("Parameter '{}': {}", name, e))?;
            match value.value {
                TextReference::Inline(source) => Ok(SchemaValue::Text(TextValuePayload {
                    text: source.data,
                    language: source.text_type.map(|text_type| text_type.language_code),
                })),
                TextReference::Url(_) => Err(format!(
                    "Parameter '{}': URL text references cannot be converted to SchemaValue",
                    name
                )),
            }
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let json_value = json_value.ok_or_else(|| format!("Missing parameter: {}", name))?;
            let value = crate::mcp::invoke::parse_unstructured_binary(json_value, descriptor)
                .map_err(|e| format!("Parameter '{}': {}", name, e))?;
            match value.value {
                BinaryReference::Inline(source) => Ok(SchemaValue::Binary(BinaryValuePayload {
                    bytes: source.data,
                    mime_type: Some(source.binary_type.mime_type),
                })),
                BinaryReference::Url(_) => Err(format!(
                    "Parameter '{}': URL binary references cannot be converted to SchemaValue",
                    name
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::agent::{
        BinaryDescriptor, ComponentModelElementSchema, NamedElementSchemas, TextDescriptor,
    };
    use golem_wasm::analysis::analysed_type::{option, str};
    use serde_json::json;
    use test_r::test;

    fn string_schema(name: &str) -> NamedElementSchema {
        NamedElementSchema {
            name: name.to_string(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: str(),
            }),
        }
    }

    fn text_schema(name: &str) -> NamedElementSchema {
        NamedElementSchema {
            name: name.to_string(),
            schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
        }
    }

    fn binary_schema(name: &str) -> NamedElementSchema {
        NamedElementSchema {
            name: name.to_string(),
            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
        }
    }

    #[test]
    fn tuple_extracts_component_model_param() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![string_schema("city")],
        });
        let args: JsonObject = json!({"city": "Sydney"}).as_object().unwrap().clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        assert!(matches!(result, SchemaValue::Record { fields } if fields.len() == 1));
    }

    #[test]
    fn tuple_extracts_unstructured_text() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![text_schema("report")],
        });
        let args: JsonObject = json!({"report": {"text": "hello world"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Text(t) => assert_eq!(t.text, "hello world"),
                _ => panic!("expected unstructured text"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn tuple_extracts_unstructured_binary() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![binary_schema("image")],
        });
        // base64url-no-pad("abc") = "YWJj"
        let args: JsonObject = json!({"image": {"bytes": "YWJj", "mime_type": "image/png"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Binary(b) => {
                    assert_eq!(b.bytes, b"abc");
                    assert_eq!(b.mime_type.as_deref(), Some("image/png"));
                }
                _ => panic!("expected unstructured binary"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn error_on_missing_required_param() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![string_schema("city")],
        });
        let args: JsonObject = json!({}).as_object().unwrap().clone();
        let err = get_agent_method_input(&args, &schema).unwrap_err();
        assert!(err.contains("Missing parameter: city"), "got: {err}");
    }

    #[test]
    fn error_on_invalid_base64() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![binary_schema("image")],
        });
        let args: JsonObject =
            json!({"image": {"bytes": "not-valid-b64!!!", "mime_type": "image/png"}})
                .as_object()
                .unwrap()
                .clone();
        let err = get_agent_method_input(&args, &schema).unwrap_err();
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn binary_without_mime_type_is_accepted() {
        // Canonical binary makes `mime_type` optional; an absent MIME type maps
        // to an empty legacy `BinaryType.mime_type`.
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![binary_schema("image")],
        });
        let args: JsonObject = json!({"image": {"bytes": "YWJj"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Binary(b) => {
                    assert_eq!(b.bytes, b"abc");
                    assert_eq!(b.mime_type.as_deref(), Some(""));
                }
                _ => panic!("expected unstructured binary"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn multimodal_extracts_parts() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![text_schema("description"), binary_schema("photo")],
        });
        let args: JsonObject = json!({
            "parts": [
                {"description": {"text": "a photo"}},
                {"photo": {"bytes": "AQID", "mime_type": "image/png"}}
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::List { elements } => assert_eq!(elements.len(), 2),
                _ => panic!("expected multimodal list"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn multimodal_error_on_unknown_part_name() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![text_schema("description")],
        });
        let args: JsonObject = json!({
            "parts": [
                {"unknown_field": {"text": "hello"}}
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let err = get_agent_method_input(&args, &schema).unwrap_err();
        assert!(err.contains("unknown element name"), "got: {err}");
    }

    #[test]
    fn tuple_missing_optional_param_defaults_to_none() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "note".to_string(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: option(str()),
                }),
            }],
        });
        let args: JsonObject = json!({}).as_object().unwrap().clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        assert!(matches!(result, SchemaValue::Record { fields } if fields.len() == 1));
    }
}
