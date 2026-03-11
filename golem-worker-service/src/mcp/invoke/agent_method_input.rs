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
use base64::Engine;
use golem_common::base_model::agent::{
    BinaryReference, BinaryReferenceValue, BinarySource, BinaryType, ComponentModelElementSchema,
    DataSchema, ElementSchema, NamedElementSchema, TextReference, TextReferenceValue, TextSource,
    TextType, UntypedDataValue, UntypedElementValue, UntypedNamedElementValue,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::model::JsonObject;

pub fn get_agent_method_input(
    mcp_args: &JsonObject,
    schema: &DataSchema,
) -> Result<UntypedDataValue, String> {
    match schema {
        DataSchema::Tuple(named_schemas) => {
            let elements = extract_element_values(mcp_args, &named_schemas.elements)?;
            Ok(UntypedDataValue::Tuple(elements))
        }
        DataSchema::Multimodal(named_schemas) => {
            let parts_array = mcp_args
                .get("parts")
                .and_then(|v| v.as_array())
                .ok_or("Multimodal input requires a parts array field")?;

            let schema_map: std::collections::HashMap<&str, &ElementSchema> = named_schemas
                .elements
                .iter()
                .map(|s| (s.name.as_str(), &s.schema))
                .collect();

            let mut named_elements = Vec::new();
            for (i, part) in parts_array.iter().enumerate() {
                let obj = part.as_object().ok_or_else(|| {
                    format!("parts[{}] must be an object with 'name' and 'value'", i)
                })?;

                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("parts[{}] is missing 'name' string field", i))?;

                let elem_schema = schema_map.get(name).ok_or_else(|| {
                    format!(
                        "parts[{}]: unknown element name '{}'. Expected one of: {}",
                        i,
                        name,
                        schema_map.keys().copied().collect::<Vec<_>>().join(", ")
                    )
                })?;

                let value_json = obj
                    .get("value")
                    .ok_or_else(|| format!("parts[{}] is missing 'value' field", i))?;

                let element = extract_multimodal_element_value(name, value_json, elem_schema, i)?;

                named_elements.push(UntypedNamedElementValue {
                    name: name.to_string(),
                    value: element,
                });
            }
            Ok(UntypedDataValue::Multimodal(named_elements))
        }
    }
}

fn extract_element_values(
    args_map: &JsonObject,
    schemas: &[NamedElementSchema],
) -> Result<Vec<UntypedElementValue>, String> {
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
) -> Result<UntypedElementValue, String> {
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

            let value_and_type =
                golem_wasm::ValueAndType::parse_with_type(&json_value, element_type).map_err(
                    |errs| format!("Failed to parse parameter '{}': {}", name, errs.join(", ")),
                )?;

            Ok(UntypedElementValue::ComponentModel(value_and_type.value))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let obj = match json_value {
                Some(serde_json::Value::Object(o)) => o,
                Some(_) => {
                    return Err(format!(
                        "Parameter '{}' must be an object with 'data' and optional 'languageCode'",
                        name
                    ));
                }
                None => return Err(format!("Missing parameter: {}", name)),
            };

            let data = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("Parameter '{}' is missing 'data' string field", name))?
                .to_string();

            let language_code = obj.get("languageCode").and_then(|v| v.as_str());

            if let Some(code) = language_code
                && let Some(allowed) = &descriptor.restrictions
                && !allowed.is_empty()
                && !allowed.iter().any(|t| t.language_code == code)
            {
                let expected: Vec<&str> =
                    allowed.iter().map(|t| t.language_code.as_str()).collect();
                return Err(format!(
                    "Parameter '{}': language code '{}' is not allowed. Expected one of: {}",
                    name,
                    code,
                    expected.join(", ")
                ));
            }

            let text_type = language_code.map(|code| TextType {
                language_code: code.to_string(),
            });

            Ok(UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource { data, text_type }),
            }))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let obj = match json_value {
                Some(serde_json::Value::Object(o)) => o,
                Some(_) => {
                    return Err(format!(
                        "Parameter '{}' must be an object with 'data' and 'mimeType'",
                        name
                    ));
                }
                None => return Err(format!("Missing parameter: {}", name)),
            };

            let b64 = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("Parameter '{}' is missing 'data' string field", name))?;

            let mime_type = obj
                .get("mimeType")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!("Parameter '{}' is missing 'mimeType' string field", name)
                })?;

            if let Some(allowed) = &descriptor.restrictions
                && !allowed.is_empty()
                && !allowed.iter().any(|t| t.mime_type == mime_type)
            {
                let expected: Vec<&str> = allowed.iter().map(|t| t.mime_type.as_str()).collect();
                return Err(format!(
                    "Parameter '{}': MIME type '{}' is not allowed. Expected one of: {}",
                    name,
                    mime_type,
                    expected.join(", ")
                ));
            }

            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("Failed to decode base64 parameter '{}': {}", name, e))?;

            Ok(UntypedElementValue::UnstructuredBinary(
                BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data,
                        binary_type: BinaryType {
                            mime_type: mime_type.to_string(),
                        },
                    }),
                },
            ))
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
        assert!(matches!(result, UntypedDataValue::Tuple(elems) if elems.len() == 1));
    }

    #[test]
    fn tuple_extracts_unstructured_text() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![text_schema("report")],
        });
        let args: JsonObject = json!({"report": {"data": "hello world"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        match result {
            UntypedDataValue::Tuple(elems) => match &elems[0] {
                UntypedElementValue::UnstructuredText(t) => match &t.value {
                    TextReference::Inline(src) => assert_eq!(src.data, "hello world"),
                    _ => panic!("expected inline text"),
                },
                _ => panic!("expected unstructured text"),
            },
            _ => panic!("expected tuple"),
        }
    }

    #[test]
    fn tuple_extracts_unstructured_binary() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![binary_schema("image")],
        });
        // base64("abc") = "YWJj"
        let args: JsonObject = json!({"image": {"data": "YWJj", "mimeType": "image/png"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        match result {
            UntypedDataValue::Tuple(elems) => match &elems[0] {
                UntypedElementValue::UnstructuredBinary(b) => match &b.value {
                    BinaryReference::Inline(src) => {
                        assert_eq!(src.data, b"abc");
                        assert_eq!(src.binary_type.mime_type, "image/png");
                    }
                    _ => panic!("expected inline binary"),
                },
                _ => panic!("expected unstructured binary"),
            },
            _ => panic!("expected tuple"),
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
            json!({"image": {"data": "not-valid-b64!!!", "mimeType": "image/png"}})
                .as_object()
                .unwrap()
                .clone();
        let err = get_agent_method_input(&args, &schema).unwrap_err();
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn error_on_binary_missing_mime_type() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![binary_schema("image")],
        });
        let args: JsonObject = json!({"image": {"data": "YWJj"}})
            .as_object()
            .unwrap()
            .clone();
        let err = get_agent_method_input(&args, &schema).unwrap_err();
        assert!(err.contains("mimeType"), "got: {err}");
    }

    #[test]
    fn multimodal_extracts_parts() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![text_schema("description"), binary_schema("photo")],
        });
        let args: JsonObject = json!({
            "parts": [
                {"name": "description", "value": {"data": "a photo"}},
                {"name": "photo", "value": {"data": "AQID", "mimeType": "image/png"}}
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let result = get_agent_method_input(&args, &schema).unwrap();
        assert!(matches!(result, UntypedDataValue::Multimodal(parts) if parts.len() == 2));
    }

    #[test]
    fn multimodal_error_on_unknown_part_name() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![text_schema("description")],
        });
        let args: JsonObject = json!({
            "parts": [
                {"name": "unknown_field", "value": {"data": "hello"}}
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
        assert!(matches!(result, UntypedDataValue::Tuple(elems) if elems.len() == 1));
    }
}
