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

use crate::mcp::invoke::{schema_binary_value_from_json, schema_text_value_from_json};
use golem_common::schema::adapters::resolve_ref;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render::json_value::from_json_value;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;

/// Extract a single multimodal part value, typed by the multimodal variant
/// case's payload schema (resolved against `graph`).
///
/// `Text` / `Binary` payloads are parsed from their canonical JSON envelopes
/// (with restriction enforcement); every other payload is a component-model
/// value parsed through the shared schema-layer JSON codec.
pub fn extract_multimodal_element_value(
    name: &str,
    value_json: &serde_json::Value,
    case_schema: &SchemaType,
    graph: &SchemaGraph,
    index: usize,
) -> Result<SchemaValue, String> {
    let resolved = resolve_ref(graph, case_schema)
        .map_err(|e| format!("parts[{}] '{}': {}", index, name, e))?;
    match resolved {
        SchemaType::Text { restrictions, .. } => {
            schema_text_value_from_json(value_json, restrictions)
                .map_err(|e| format!("parts[{}] '{}': {}", index, name, e))
        }
        SchemaType::Binary { restrictions, .. } => {
            schema_binary_value_from_json(value_json, restrictions)
                .map_err(|e| format!("parts[{}] '{}': {}", index, name, e))
        }
        _ => from_json_value(graph, case_schema, value_json)
            .map_err(|e| format!("parts[{}] '{}': failed to parse value: {}", index, name, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::schema_type::{BinaryRestrictions, SchemaType, TextRestrictions};
    use serde_json::json;
    use test_r::test;

    fn graph() -> SchemaGraph {
        SchemaGraph::empty()
    }

    #[test]
    fn extracts_component_model_string() {
        let schema = SchemaType::string();
        let value = json!("hello");
        let result = extract_multimodal_element_value("msg", &value, &schema, &graph(), 0).unwrap();
        assert!(matches!(result, SchemaValue::String(s) if s == "hello"));
    }

    #[test]
    fn extracts_text_without_language_code() {
        let schema = SchemaType::text(TextRestrictions::default());
        let value = json!({"text": "some text"});
        let result =
            extract_multimodal_element_value("note", &value, &schema, &graph(), 0).unwrap();
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
        let schema = SchemaType::text(TextRestrictions::default());
        let value = json!({"text": "bonjour", "language": "fr"});
        let result =
            extract_multimodal_element_value("note", &value, &schema, &graph(), 0).unwrap();
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
        let schema = SchemaType::text(TextRestrictions {
            languages: Some(vec!["en".to_string()]),
            ..Default::default()
        });
        let value = json!({"text": "hola", "language": "es"});
        let err =
            extract_multimodal_element_value("note", &value, &schema, &graph(), 1).unwrap_err();
        assert!(
            err.contains("language code 'es' is not allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn extracts_binary() {
        let schema = SchemaType::binary(BinaryRestrictions::default());
        let value = json!({"bytes": "AQID", "mime_type": "image/png"});
        let result = extract_multimodal_element_value("img", &value, &schema, &graph(), 0).unwrap();
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
        let schema = SchemaType::binary(BinaryRestrictions {
            mime_types: Some(vec!["image/png".to_string()]),
            ..Default::default()
        });
        let value = json!({"bytes": "AQID", "mime_type": "image/jpeg"});
        let err =
            extract_multimodal_element_value("img", &value, &schema, &graph(), 0).unwrap_err();
        assert!(
            err.contains("MIME type 'image/jpeg' is not allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn error_on_invalid_base64() {
        let schema = SchemaType::binary(BinaryRestrictions::default());
        let value = json!({"bytes": "!!!invalid!!!", "mime_type": "image/png"});
        let err =
            extract_multimodal_element_value("img", &value, &schema, &graph(), 0).unwrap_err();
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn error_on_missing_text_field() {
        let schema = SchemaType::text(TextRestrictions::default());
        let value = json!({"other": "stuff"});
        let err =
            extract_multimodal_element_value("note", &value, &schema, &graph(), 0).unwrap_err();
        assert!(err.contains("text"), "got: {err}");
    }

    #[test]
    fn error_on_missing_bytes_field() {
        let schema = SchemaType::binary(BinaryRestrictions::default());
        let value = json!({"mime_type": "image/png"});
        let err =
            extract_multimodal_element_value("img", &value, &schema, &graph(), 0).unwrap_err();
        assert!(err.contains("bytes"), "got: {err}");
    }
}
