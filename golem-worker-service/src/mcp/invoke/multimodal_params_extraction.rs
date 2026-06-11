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
    BinaryReference, ComponentModelElementSchema, ElementSchema, TextReference,
};
use golem_common::schema::adapters::value_to_schema_value;
use golem_common::schema::{BinaryValuePayload, SchemaValue, TextValuePayload};

pub fn extract_multimodal_element_value(
    name: &str,
    value_json: &serde_json::Value,
    elem_schema: &ElementSchema,
    index: usize,
) -> Result<SchemaValue, String> {
    match elem_schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let value = crate::mcp::invoke::parse_component_model_value(value_json, element_type)
                .map_err(|e| {
                format!("parts[{}] '{}': failed to parse value: {}", index, name, e)
            })?;
            value_to_schema_value(&value, element_type).map_err(|e| {
                format!(
                    "parts[{}] '{}': failed to convert value: {}",
                    index, name, e
                )
            })
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let value = crate::mcp::invoke::parse_unstructured_text(value_json, descriptor)
                .map_err(|e| format!("parts[{}] '{}': {}", index, name, e))?;
            match value.value {
                TextReference::Inline(source) => Ok(SchemaValue::Text(TextValuePayload {
                    text: source.data,
                    language: source.text_type.map(|text_type| text_type.language_code),
                })),
                TextReference::Url(_) => Err(format!(
                    "parts[{}] '{}': URL text references cannot be converted to SchemaValue",
                    index, name
                )),
            }
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let value = crate::mcp::invoke::parse_unstructured_binary(value_json, descriptor)
                .map_err(|e| format!("parts[{}] '{}': {}", index, name, e))?;
            match value.value {
                BinaryReference::Inline(source) => Ok(SchemaValue::Binary(BinaryValuePayload {
                    bytes: source.data,
                    mime_type: Some(source.binary_type.mime_type),
                })),
                BinaryReference::Url(_) => Err(format!(
                    "parts[{}] '{}': URL binary references cannot be converted to SchemaValue",
                    index, name
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::agent::{BinaryDescriptor, BinaryType, TextDescriptor, TextType};
    use golem_wasm::analysis::analysed_type::str;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn extracts_component_model_string() {
        let schema = ElementSchema::ComponentModel(ComponentModelElementSchema {
            element_type: str(),
        });
        let value = json!("hello");
        let result = extract_multimodal_element_value("msg", &value, &schema, 0).unwrap();
        assert!(matches!(result, SchemaValue::String(s) if s == "hello"));
    }

    #[test]
    fn extracts_text_without_language_code() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
        let value = json!({"text": "some text"});
        let result = extract_multimodal_element_value("note", &value, &schema, 0).unwrap();
        match result {
            SchemaValue::Text(t) => {
                assert_eq!(t.text, "some text");
                assert!(t.language.is_none());
            }
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn extracts_text_with_language_code() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
        let value = json!({"text": "bonjour", "language": "fr"});
        let result = extract_multimodal_element_value("note", &value, &schema, 0).unwrap();
        match result {
            SchemaValue::Text(t) => {
                assert_eq!(t.text, "bonjour");
                assert_eq!(t.language.unwrap(), "fr");
            }
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn rejects_disallowed_language_code() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor {
            restrictions: Some(vec![TextType {
                language_code: "en".to_string(),
            }]),
        });
        let value = json!({"text": "hola", "language": "es"});
        let err = extract_multimodal_element_value("note", &value, &schema, 1).unwrap_err();
        assert!(
            err.contains("language code 'es' is not allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn extracts_binary() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
        let value = json!({"bytes": "AQID", "mime_type": "image/png"});
        let result = extract_multimodal_element_value("img", &value, &schema, 0).unwrap();
        match result {
            SchemaValue::Binary(b) => {
                assert_eq!(b.bytes, vec![1, 2, 3]);
                assert_eq!(b.mime_type.as_deref(), Some("image/png"));
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn rejects_disallowed_mime_type() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor {
            restrictions: Some(vec![BinaryType {
                mime_type: "image/png".to_string(),
            }]),
        });
        let value = json!({"bytes": "AQID", "mime_type": "image/jpeg"});
        let err = extract_multimodal_element_value("img", &value, &schema, 0).unwrap_err();
        assert!(
            err.contains("MIME type 'image/jpeg' is not allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn error_on_invalid_base64() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
        let value = json!({"bytes": "!!!invalid!!!", "mime_type": "image/png"});
        let err = extract_multimodal_element_value("img", &value, &schema, 0).unwrap_err();
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn error_on_missing_text_field() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
        let value = json!({"other": "stuff"});
        let err = extract_multimodal_element_value("note", &value, &schema, 0).unwrap_err();
        assert!(err.contains("text"), "got: {err}");
    }

    #[test]
    fn error_on_missing_bytes_field() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
        let value = json!({"mime_type": "image/png"});
        let err = extract_multimodal_element_value("img", &value, &schema, 0).unwrap_err();
        assert!(err.contains("bytes"), "got: {err}");
    }
}
