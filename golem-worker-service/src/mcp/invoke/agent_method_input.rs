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
use crate::mcp::invoke::{schema_binary_value_from_json, schema_text_value_from_json};
use golem_common::schema::adapters::{multimodal_variant_cases, resolve_ref};
use golem_common::schema::agent::{FieldSource, InputSchema, NamedField};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render::json_value::from_json_value;
use golem_common::schema::schema_type::{SchemaType, VariantCaseType};
use golem_common::schema::schema_value::{SchemaValue, VariantValuePayload};
use rmcp::model::JsonObject;
use std::collections::HashMap;

/// Build the method input record for an MCP invocation from the advertised
/// arguments object and the method's [`InputSchema`].
///
/// Only [`FieldSource::UserSupplied`] fields are read from the arguments (the
/// host fills auto-injected fields out of band); a multimodal input — a single
/// user-supplied field whose schema is the structural form
/// `list<variant<… Role::Multimodal>>` — is read from the `parts` array.
pub fn get_agent_method_input(
    mcp_args: &JsonObject,
    graph: &SchemaGraph,
    input: &InputSchema,
) -> Result<SchemaValue, String> {
    let user_fields: Vec<&NamedField> = input
        .fields()
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .collect();

    // Structural multimodal input: a single user-supplied field whose schema is
    // `list<variant<… Role::Multimodal>>`.
    if let [field] = user_fields.as_slice()
        && let Some(cases) =
            multimodal_variant_cases(graph, &field.schema).map_err(|e| e.to_string())?
    {
        return extract_multimodal_input(mcp_args, graph, cases);
    }

    let mut fields = Vec::with_capacity(user_fields.len());
    for field in user_fields {
        fields.push(extract_single_field_value(mcp_args, graph, field)?);
    }
    Ok(SchemaValue::Record { fields })
}

fn extract_multimodal_input(
    mcp_args: &JsonObject,
    graph: &SchemaGraph,
    cases: &[VariantCaseType],
) -> Result<SchemaValue, String> {
    let parts_array = mcp_args
        .get("parts")
        .and_then(|v| v.as_array())
        .ok_or("Multimodal input requires a parts array field")?;

    let schema_map: HashMap<&str, (usize, &SchemaType)> = cases
        .iter()
        .enumerate()
        .filter_map(|(case, c)| c.payload.as_ref().map(|p| (c.name.as_str(), (case, p))))
        .collect();

    let mut elements = Vec::new();
    for (i, part) in parts_array.iter().enumerate() {
        // Each multimodal part is a canonical variant object: a single-key
        // object `{ <element name>: <value> }`.
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

        let (case, payload_schema) = schema_map.get(name.as_str()).ok_or_else(|| {
            format!(
                "parts[{}]: unknown element name '{}'. Expected one of: {}",
                i,
                name,
                schema_map.keys().copied().collect::<Vec<_>>().join(", ")
            )
        })?;

        let body = extract_multimodal_element_value(name, value_json, payload_schema, graph, i)?;

        elements.push(SchemaValue::Variant(VariantValuePayload {
            case: *case as u32,
            payload: Some(Box::new(body)),
        }));
    }
    Ok(SchemaValue::Record {
        fields: vec![SchemaValue::List { elements }],
    })
}

fn extract_single_field_value(
    args_map: &JsonObject,
    graph: &SchemaGraph,
    field: &NamedField,
) -> Result<SchemaValue, String> {
    let name = &field.name;
    let resolved = resolve_ref(graph, &field.schema).map_err(|e| e.to_string())?;
    match resolved {
        SchemaType::Text { restrictions, .. } => {
            let json_value = args_map
                .get(name)
                .ok_or_else(|| format!("Missing parameter: {}", name))?;
            schema_text_value_from_json(json_value, restrictions)
                .map_err(|e| format!("Parameter '{}': {}", name, e))
        }
        SchemaType::Binary { restrictions, .. } => {
            let json_value = args_map
                .get(name)
                .ok_or_else(|| format!("Missing parameter: {}", name))?;
            schema_binary_value_from_json(json_value, restrictions)
                .map_err(|e| format!("Parameter '{}': {}", name, e))
        }
        _ => {
            let json_value = match args_map.get(name) {
                Some(value) => value.clone(),
                None => {
                    if matches!(resolved, SchemaType::Option { .. }) {
                        serde_json::Value::Null
                    } else {
                        return Err(format!("Missing parameter: {}", name));
                    }
                }
            };
            from_json_value(graph, &field.schema, &json_value)
                .map_err(|e| format!("Failed to parse parameter '{}': {}", name, e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::adapters::{
        MULTIMODAL_PARTS_FIELD_NAME, unstructured_binary_schema_type, unstructured_text_schema_type,
    };
    use golem_common::schema::metadata::Role;
    use golem_common::schema::schema_type::{BinaryRestrictions, SchemaType, TextRestrictions};
    use serde_json::json;
    use test_r::test;

    fn graph() -> SchemaGraph {
        SchemaGraph::empty()
    }

    fn string_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::string())
    }

    fn text_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::text(TextRestrictions::default()))
    }

    fn binary_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::binary(BinaryRestrictions::default()))
    }

    fn restricted_binary_field(name: &str, mime_types: Vec<&str>) -> NamedField {
        NamedField::user_supplied(
            name,
            SchemaType::binary(BinaryRestrictions {
                mime_types: Some(mime_types.into_iter().map(String::from).collect()),
                ..Default::default()
            }),
        )
    }

    /// Build a structural multimodal input field (`list<variant<… Role::Multimodal>>`).
    fn multimodal_field(cases: Vec<(&str, SchemaType)>) -> NamedField {
        let variant_cases = cases
            .into_iter()
            .map(|(name, ty)| VariantCaseType {
                name: name.to_string(),
                payload: Some(ty),
                metadata: Default::default(),
            })
            .collect();
        let mut list = SchemaType::list(SchemaType::variant(variant_cases));
        list.metadata_mut().role = Some(Role::Multimodal);
        NamedField::user_supplied(MULTIMODAL_PARTS_FIELD_NAME, list)
    }

    fn input(fields: Vec<NamedField>) -> InputSchema {
        InputSchema::Parameters(fields)
    }

    #[test]
    fn tuple_extracts_component_model_param() {
        let schema = input(vec![string_field("city")]);
        let args: JsonObject = json!({"city": "Sydney"}).as_object().unwrap().clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        assert!(matches!(result, SchemaValue::Record { fields } if fields.len() == 1));
    }

    #[test]
    fn tuple_extracts_unstructured_text() {
        let schema = input(vec![text_field("report")]);
        let args: JsonObject = json!({"report": {"text": "hello world"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
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
        let schema = input(vec![binary_field("image")]);
        // base64url-no-pad("abc") = "YWJj"
        let args: JsonObject = json!({"image": {"bytes": "YWJj", "mime_type": "image/png"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
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
        let schema = input(vec![string_field("city")]);
        let args: JsonObject = json!({}).as_object().unwrap().clone();
        let err = get_agent_method_input(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("Missing parameter: city"), "got: {err}");
    }

    #[test]
    fn error_on_invalid_base64() {
        let schema = input(vec![binary_field("image")]);
        let args: JsonObject =
            json!({"image": {"bytes": "not-valid-b64!!!", "mime_type": "image/png"}})
                .as_object()
                .unwrap()
                .clone();
        let err = get_agent_method_input(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn binary_without_mime_type_is_accepted() {
        // Canonical binary makes `mime_type` optional; an absent MIME type maps
        // to `None` in the schema-native value.
        let schema = input(vec![binary_field("image")]);
        let args: JsonObject = json!({"image": {"bytes": "YWJj"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Binary(b) => {
                    assert_eq!(b.bytes, b"abc");
                    assert!(b.mime_type.is_none());
                }
                _ => panic!("expected unstructured binary"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn restricted_binary_without_mime_type_is_accepted() {
        // Lenient MIME handling (D2): even when the schema restricts the MIME
        // type, an absent MIME type on the value is allowed — only a *present*
        // MIME type outside the allow-list is rejected.
        let schema = input(vec![restricted_binary_field("image", vec!["image/png"])]);
        let args: JsonObject = json!({"image": {"bytes": "YWJj"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Binary(b) => {
                    assert_eq!(b.bytes, b"abc");
                    assert!(b.mime_type.is_none());
                }
                _ => panic!("expected unstructured binary"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn restricted_binary_with_disallowed_mime_type_is_rejected() {
        let schema = input(vec![restricted_binary_field("image", vec!["image/png"])]);
        let args: JsonObject = json!({"image": {"bytes": "YWJj", "mime_type": "image/jpeg"}})
            .as_object()
            .unwrap()
            .clone();
        let err = get_agent_method_input(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("image/jpeg' is not allowed"), "got: {err}");
    }

    #[test]
    fn multimodal_extracts_parts() {
        let schema = input(vec![multimodal_field(vec![
            ("description", SchemaType::text(TextRestrictions::default())),
            ("photo", SchemaType::binary(BinaryRestrictions::default())),
        ])]);
        let args: JsonObject = json!({
            "parts": [
                {"description": {"text": "a photo"}},
                {"photo": {"bytes": "AQID", "mime_type": "image/png"}}
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
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
        let schema = input(vec![multimodal_field(vec![(
            "description",
            SchemaType::text(TextRestrictions::default()),
        )])]);
        let args: JsonObject = json!({
            "parts": [
                {"unknown_field": {"text": "hello"}}
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let err = get_agent_method_input(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("unknown element name"), "got: {err}");
    }

    /// A canonical role-marked unstructured-text wrapper input field
    /// (`variant { inline: text, url }`). Real guest SDK agents publish the
    /// wrapper form, so the advertised MCP input schema renders it as a generic
    /// variant `oneOf` and the extraction path parses the same single-key object
    /// form via `from_json_value` — there is no bare-scalar ergonomic shortcut
    /// for the wrapper.
    fn wrapper_text_field(name: &str) -> NamedField {
        NamedField::user_supplied(
            name,
            unstructured_text_schema_type(TextRestrictions::default()),
        )
    }

    fn wrapper_binary_field(name: &str) -> NamedField {
        NamedField::user_supplied(
            name,
            unstructured_binary_schema_type(BinaryRestrictions::default()),
        )
    }

    #[test]
    fn wrapper_text_input_inline_case_roundtrips() {
        let schema = input(vec![wrapper_text_field("report")]);
        // Canonical variant wire form (what the advertised oneOf schema declares).
        let args: JsonObject = json!({"report": {"inline": {"text": "hello world"}}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Variant(VariantValuePayload { case, payload }) => {
                    assert_eq!(*case, 0); // inline
                    match payload.as_deref() {
                        Some(SchemaValue::Text(t)) => assert_eq!(t.text, "hello world"),
                        _ => panic!("expected inline text payload"),
                    }
                }
                _ => panic!("expected wrapper variant"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn wrapper_text_input_url_case_roundtrips() {
        let schema = input(vec![wrapper_text_field("report")]);
        let args: JsonObject = json!({"report": {"url": "https://example.com/report.txt"}})
            .as_object()
            .unwrap()
            .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Variant(VariantValuePayload { case, payload }) => {
                    assert_eq!(*case, 1); // url
                    match payload.as_deref() {
                        Some(SchemaValue::Url { url }) => {
                            assert_eq!(url, "https://example.com/report.txt")
                        }
                        _ => panic!("expected url payload"),
                    }
                }
                _ => panic!("expected wrapper variant"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn wrapper_binary_input_inline_case_roundtrips() {
        let schema = input(vec![wrapper_binary_field("image")]);
        let args: JsonObject =
            json!({"image": {"inline": {"bytes": "YWJj", "mime_type": "image/png"}}})
                .as_object()
                .unwrap()
                .clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        match result {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::Variant(VariantValuePayload { case, payload }) => {
                    assert_eq!(*case, 0); // inline
                    match payload.as_deref() {
                        Some(SchemaValue::Binary(b)) => {
                            assert_eq!(b.bytes, b"abc");
                            assert_eq!(b.mime_type.as_deref(), Some("image/png"));
                        }
                        _ => panic!("expected inline binary payload"),
                    }
                }
                _ => panic!("expected wrapper variant"),
            },
            _ => panic!("expected record"),
        }
    }

    #[test]
    fn tuple_missing_optional_param_defaults_to_none() {
        let schema = input(vec![NamedField::user_supplied(
            "note",
            SchemaType::option(SchemaType::string()),
        )]);
        let args: JsonObject = json!({}).as_object().unwrap().clone();
        let result = get_agent_method_input(&args, &graph(), &schema).unwrap();
        assert!(matches!(result, SchemaValue::Record { fields } if fields.len() == 1));
    }
}
