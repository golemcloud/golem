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

use golem_common::schema::agent::{FieldSource, InputSchema, NamedField};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::multimodal::multimodal_variant_cases;
use golem_common::schema::render::json_value::from_json_value;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;

/// Validate that a constructor [`InputSchema`] can be supplied through MCP,
/// without requiring actual argument values. This mirrors the structural rules
/// enforced by [`extract_constructor_input_values`] and is used at export time
/// so we never advertise a tool/resource whose constructor could never be
/// satisfied via MCP (multimodal / unstructured constructor parameters).
pub fn validate_constructor_schema_for_mcp(
    graph: &SchemaGraph,
    input: &InputSchema,
) -> Result<(), String> {
    let user_fields = user_supplied_fields(input);
    reject_multimodal(graph, &user_fields)?;
    for field in user_fields {
        ensure_supplyable_via_mcp(graph, field)?;
    }
    Ok(())
}

/// Extract the user-supplied constructor parameters from the advertised
/// arguments object, in field order, as positional [`SchemaValue`]s. The
/// caller wraps these into the agent-id's parameter record.
pub fn extract_constructor_input_values(
    args_map: &rmcp::model::JsonObject,
    graph: &SchemaGraph,
    input: &InputSchema,
) -> Result<Vec<SchemaValue>, String> {
    let user_fields = user_supplied_fields(input);
    reject_multimodal(graph, &user_fields)?;

    let mut params = Vec::with_capacity(user_fields.len());
    for field in user_fields {
        ensure_supplyable_via_mcp(graph, field)?;

        let resolved = graph
            .resolve_ref(&field.schema)
            .map_err(|e| e.to_string())?;
        let json_value = match args_map.get(&field.name) {
            Some(value) => value.clone(),
            None => {
                if matches!(resolved, SchemaType::Option { .. }) {
                    serde_json::Value::Null
                } else {
                    return Err(format!("Missing parameter: {}", field.name));
                }
            }
        };

        let value = from_json_value(graph, &field.schema, &json_value)
            .map_err(|e| format!("Failed to parse parameter '{}': {}", field.name, e))?;
        params.push(value);
    }
    Ok(params)
}

fn user_supplied_fields(input: &InputSchema) -> Vec<&NamedField> {
    input
        .fields()
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .collect()
}

fn reject_multimodal(graph: &SchemaGraph, fields: &[&NamedField]) -> Result<(), String> {
    if let [field] = fields
        && multimodal_variant_cases(graph, &field.schema)
            .map_err(|e| e.to_string())?
            .is_some()
    {
        return Err("MCP does not support multimodal constructor schemas".to_string());
    }
    Ok(())
}

/// Reject unstructured (text/binary) constructor parameters, which cannot be
/// supplied through the MCP agent-id encoding.
fn ensure_supplyable_via_mcp(graph: &SchemaGraph, field: &NamedField) -> Result<(), String> {
    match graph
        .resolve_ref(&field.schema)
        .map_err(|e| e.to_string())?
    {
        SchemaType::Text { .. } => Err(format!(
            "MCP cannot support unstructured-text constructor parameters like '{}'",
            field.name
        )),
        SchemaType::Binary { .. } => Err(format!(
            "MCP cannot support unstructured-binary constructor parameters like '{}'",
            field.name
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::metadata::Role;
    use golem_common::schema::schema_type::{SchemaType, TextRestrictions, VariantCaseType};
    use serde_json::json;
    use test_r::test;

    fn graph() -> SchemaGraph {
        SchemaGraph::empty()
    }

    fn string_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::string())
    }

    fn u32_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::u32())
    }

    fn text_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::text(TextRestrictions::default()))
    }

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
        NamedField::user_supplied("parts", list)
    }

    fn input(fields: Vec<NamedField>) -> InputSchema {
        InputSchema::Parameters(fields)
    }

    #[test]
    fn extracts_string_param() {
        let schema = input(vec![string_field("name")]);
        let args = json!({"name": "alice"}).as_object().unwrap().clone();
        let result = extract_constructor_input_values(&args, &graph(), &schema).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn extracts_multiple_params() {
        let schema = input(vec![string_field("name"), u32_field("age")]);
        let args = json!({"name": "alice", "age": 30})
            .as_object()
            .unwrap()
            .clone();
        let result = extract_constructor_input_values(&args, &graph(), &schema).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn error_on_missing_required_param() {
        let schema = input(vec![string_field("name")]);
        let args = json!({}).as_object().unwrap().clone();
        let err = extract_constructor_input_values(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("Missing parameter: name"), "got: {err}");
    }

    #[test]
    fn rejects_unstructured_text_constructor() {
        let schema = input(vec![text_field("desc")]);
        let args = json!({"desc": "hello"}).as_object().unwrap().clone();
        let err = extract_constructor_input_values(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("unstructured-text"), "got: {err}");
    }

    #[test]
    fn rejects_multimodal_schema() {
        let schema = input(vec![multimodal_field(vec![("x", SchemaType::string())])]);
        let args = json!({}).as_object().unwrap().clone();
        let err = extract_constructor_input_values(&args, &graph(), &schema).unwrap_err();
        assert!(err.contains("multimodal"), "got: {err}");
    }
}
