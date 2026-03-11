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

use base64::Engine;
use golem_common::base_model::agent::{
    BinaryReference, BinaryReferenceValue, BinarySource, BinaryType, ComponentModelElementSchema,
    ElementSchema, TextReference, TextReferenceValue, TextSource, TextType, UntypedElementValue,
};
use golem_wasm::json::ValueAndTypeJsonExtensions;

pub fn extract_multimodal_element_value(
    name: &str,
    value_json: &serde_json::Value,
    elem_schema: &ElementSchema,
    index: usize,
) -> Result<UntypedElementValue, String> {
    match elem_schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let value_and_type =
                golem_wasm::ValueAndType::parse_with_type(value_json, element_type).map_err(
                    |errs| {
                        format!(
                            "parts[{}] '{}': failed to parse value: {}",
                            index,
                            name,
                            errs.join(", ")
                        )
                    },
                )?;
            Ok(UntypedElementValue::ComponentModel(value_and_type.value))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let obj = value_json.as_object().ok_or_else(|| {
                format!(
                    "parts[{}] '{}': value must be an object with 'data' and optional 'languageCode'",
                    index, name
                )
            })?;

            let data = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("parts[{}] '{}': missing 'data' string field", index, name))?
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
                    "parts[{}] '{}': language code '{}' is not allowed. Expected one of: {}",
                    index,
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
            let obj = value_json.as_object().ok_or_else(|| {
                format!(
                    "parts[{}] '{}': value must be an object with 'data' and 'mimeType'",
                    index, name
                )
            })?;

            let b64 = obj.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
                format!("parts[{}] '{}': missing 'data' string field", index, name)
            })?;

            let mime_type = obj
                .get("mimeType")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!(
                        "parts[{}] '{}': missing 'mimeType' string field",
                        index, name
                    )
                })?;

            if let Some(allowed) = &descriptor.restrictions
                && !allowed.is_empty()
                && !allowed.iter().any(|t| t.mime_type == mime_type)
            {
                let expected: Vec<&str> = allowed.iter().map(|t| t.mime_type.as_str()).collect();
                return Err(format!(
                    "parts[{}] '{}': MIME type '{}' is not allowed. Expected one of: {}",
                    index,
                    name,
                    mime_type,
                    expected.join(", ")
                ));
            }

            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    format!(
                        "parts[{}] '{}': failed to decode base64: {}",
                        index, name, e
                    )
                })?;

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
    use golem_common::base_model::agent::{BinaryDescriptor, TextDescriptor, TextType};
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
        assert!(matches!(result, UntypedElementValue::ComponentModel(_)));
    }

    #[test]
    fn extracts_text_without_language_code() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
        let value = json!({"data": "some text"});
        let result = extract_multimodal_element_value("note", &value, &schema, 0).unwrap();
        match result {
            UntypedElementValue::UnstructuredText(t) => match t.value {
                TextReference::Inline(src) => {
                    assert_eq!(src.data, "some text");
                    assert!(src.text_type.is_none());
                }
                _ => panic!("expected inline"),
            },
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn extracts_text_with_language_code() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
        let value = json!({"data": "bonjour", "languageCode": "fr"});
        let result = extract_multimodal_element_value("note", &value, &schema, 0).unwrap();
        match result {
            UntypedElementValue::UnstructuredText(t) => match t.value {
                TextReference::Inline(src) => {
                    assert_eq!(src.data, "bonjour");
                    assert_eq!(src.text_type.unwrap().language_code, "fr");
                }
                _ => panic!("expected inline"),
            },
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
        let value = json!({"data": "hola", "languageCode": "es"});
        let err = extract_multimodal_element_value("note", &value, &schema, 1).unwrap_err();
        assert!(
            err.contains("language code 'es' is not allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn extracts_binary() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
        let value = json!({"data": "AQID", "mimeType": "image/png"});
        let result = extract_multimodal_element_value("img", &value, &schema, 0).unwrap();
        match result {
            UntypedElementValue::UnstructuredBinary(b) => match b.value {
                BinaryReference::Inline(src) => {
                    assert_eq!(src.data, vec![1, 2, 3]);
                    assert_eq!(src.binary_type.mime_type, "image/png");
                }
                _ => panic!("expected inline"),
            },
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
        let value = json!({"data": "AQID", "mimeType": "image/jpeg"});
        let err = extract_multimodal_element_value("img", &value, &schema, 0).unwrap_err();
        assert!(
            err.contains("MIME type 'image/jpeg' is not allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn error_on_invalid_base64() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
        let value = json!({"data": "!!!invalid!!!", "mimeType": "image/png"});
        let err = extract_multimodal_element_value("img", &value, &schema, 0).unwrap_err();
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn error_on_missing_data_field_text() {
        let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
        let value = json!({"other": "stuff"});
        let err = extract_multimodal_element_value("note", &value, &schema, 0).unwrap_err();
        assert!(err.contains("missing 'data'"), "got: {err}");
    }

    #[test]
    fn error_on_missing_mime_type() {
        let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
        let value = json!({"data": "AQID"});
        let err = extract_multimodal_element_value("img", &value, &schema, 0).unwrap_err();
        assert!(err.contains("mimeType"), "got: {err}");
    }
}
