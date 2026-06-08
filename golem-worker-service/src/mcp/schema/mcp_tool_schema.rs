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

//! MCP tool/resource schema export, driven by the shared, configurable
//! `golem-common` JSON Schema renderer (`JsonSchemaConfig::MCP`).
//!
//! There is no MCP-specific renderer: the same renderer that produces
//! canonical JSON Schema documents is configured to emit the MCP content-block
//! shapes (`{ data, languageCode }` for text, `{ data, mimeType }` for binary,
//! and the multimodal `parts` array of `{ name, value }` objects).

use golem_common::schema::adapters::{
    FALLBACK_OUTPUT_FIELD_NAME, is_multimodal_schema_type, resolve_ref,
};
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, InputSchema, NamedField, OutputSchema,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render::{
    JsonSchemaConfig, input_schema_to_json_schema, output_schema_to_json_schema,
};
use golem_common::schema::schema_type::SchemaType;
use rmcp::model::JsonObject;
use serde_json::{Value, json};

pub struct McpToolSchema {
    /// MCP tool input schema (constructor + method parameters merged).
    pub input_schema: JsonObject,
    /// MCP tool output schema. `None` for unstructured (text/binary) and
    /// multimodal outputs, where clients render the content array instead.
    pub output_schema: Option<JsonObject>,
}

/// Build the MCP input + output JSON schemas for a method, combining the
/// constructor's user-supplied parameters with the method's.
pub fn get_mcp_tool_schema(
    graph: &SchemaGraph,
    constructor: &AgentConstructorSchema,
    method: &AgentMethodSchema,
) -> McpToolSchema {
    McpToolSchema {
        input_schema: combined_input_schema(graph, constructor, method),
        output_schema: structured_output_schema(graph, &method.output_schema),
    }
}

/// Render an input schema (constructor or method) to an MCP JSON object schema.
pub fn input_schema_to_mcp(graph: &SchemaGraph, input: &InputSchema) -> JsonObject {
    rmcp::model::object(input_schema_to_json_schema(
        graph,
        input,
        JsonSchemaConfig::MCP,
    ))
}

/// Merge the constructor's user-supplied parameters with the method's and
/// render them as a single MCP JSON object schema. Auto-injected fields are
/// dropped by the renderer; `required` is computed option-aware.
pub fn combined_input_schema(
    graph: &SchemaGraph,
    constructor: &AgentConstructorSchema,
    method: &AgentMethodSchema,
) -> JsonObject {
    let mut fields: Vec<NamedField> = Vec::new();
    fields.extend(constructor.input_schema.fields().iter().cloned());
    fields.extend(method.input_schema.fields().iter().cloned());
    let combined = InputSchema::Parameters(fields);
    input_schema_to_mcp(graph, &combined)
}

/// Render the MCP output schema, following MCP policy: only structured
/// (component-model) single outputs get an output schema. `Unit`, unstructured
/// (text/binary), and multimodal outputs return `None` so MCP clients render
/// the content array instead of validating structured content.
fn structured_output_schema(graph: &SchemaGraph, output: &OutputSchema) -> Option<JsonObject> {
    let OutputSchema::Single(ty) = output else {
        return None;
    };
    if is_unstructured_output(graph, ty) {
        return None;
    }
    let inner = output_schema_to_json_schema(graph, output, JsonSchemaConfig::MCP)?;
    // The new model carries no output element name (§4.7), so the single
    // return value is wrapped under the same synthetic key the response mapper
    // uses after the new→legacy conversion (`FALLBACK_OUTPUT_FIELD_NAME`). This
    // keeps the advertised schema and the produced `structured_content`
    // aligned and makes the output an MCP-legal `object`.
    let required = if resolves_to_option(graph, ty) {
        Vec::new()
    } else {
        vec![Value::String(FALLBACK_OUTPUT_FIELD_NAME.to_string())]
    };
    Some(rmcp::model::object(json!({
        "type": "object",
        "properties": { FALLBACK_OUTPUT_FIELD_NAME: inner },
        "required": required,
    })))
}

/// Whether the output type is unstructured (`Text` / `Binary`) or multimodal,
/// in which case MCP omits the output schema.
fn is_unstructured_output(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    // Output refs are pre-validated in `from_agent_method` (via the legacy
    // projection of the method's output schema), so this classification runs on
    // fully-resolvable refs; the `unwrap_or(false)` / `Err` fallbacks only guard
    // truly unreachable cases rather than masking real ref errors.
    if is_multimodal_schema_type(graph, ty).unwrap_or(false) {
        return true;
    }
    matches!(
        resolve_ref(graph, ty),
        Ok(SchemaType::Text { .. }) | Ok(SchemaType::Binary { .. })
    )
}

fn resolves_to_option(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(resolve_ref(graph, ty), Ok(SchemaType::Option { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::agent::{InputSchema, NamedField, OutputSchema};
    use golem_common::schema::schema_type::{SchemaType, TextRestrictions};
    use serde_json::json;
    use test_r::test;

    fn constructor(fields: Vec<NamedField>) -> AgentConstructorSchema {
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(fields),
        }
    }

    fn method(input: Vec<NamedField>, output: OutputSchema) -> AgentMethodSchema {
        AgentMethodSchema {
            name: "m".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(input),
            output_schema: output,
            http_endpoint: vec![],
            read_only: None,
        }
    }

    #[test]
    fn combined_input_merges_constructor_and_method_params() {
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![NamedField::user_supplied(
            "name",
            SchemaType::string(),
        )]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::string())),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        let props = schema.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("name"));
        assert!(props.contains_key("city"));
        let required = schema.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("city")));
    }

    #[test]
    fn structured_output_is_wrapped_under_value_key() {
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::u32())),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        let out = schema.output_schema.expect("structured output schema");
        assert_eq!(out["type"], json!("object"));
        assert_eq!(out["properties"]["value"]["type"], json!("integer"));
        assert_eq!(out["required"], json!(["value"]));
    }

    #[test]
    fn unstructured_text_output_has_no_schema() {
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::text(TextRestrictions::default()))),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        assert!(schema.output_schema.is_none());
    }

    #[test]
    fn unit_output_has_no_schema() {
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Unit,
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        assert!(schema.output_schema.is_none());
    }

    #[test]
    fn optional_output_value_is_not_required() {
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::option(SchemaType::u32()))),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        let out = schema.output_schema.expect("output schema");
        assert_eq!(out["required"], json!([]));
    }
}
