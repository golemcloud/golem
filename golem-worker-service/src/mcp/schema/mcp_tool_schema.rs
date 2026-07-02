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

//! MCP tool/resource schema export, driven by the shared `golem-common` JSON
//! Schema renderer.
//!
//! There is no MCP-specific renderer and no MCP-specific JSON shapes: the same
//! renderer that produces canonical JSON Schema documents is used here, only
//! configured to omit the `$schema` draft marker
//! (`JsonSchemaConfig::WITHOUT_DRAFT_MARKER`) since tool/resource schemas are
//! embedded rather than standalone. Text renders as `{ text, language? }`,
//! binary as `{ bytes (base64url), mime_type? }`, and multimodal lists as a
//! `parts` array of canonical variant objects `{ <caseName>: <payload> }`.

use crate::mcp::schema::field_disambiguation::field_name_mapping;
use golem_common::schema::FALLBACK_OUTPUT_FIELD_NAME;
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, InputSchema, NamedField, OutputSchema,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::multimodal::is_multimodal_schema_type;
use golem_common::schema::render::{
    JsonSchemaConfig, input_schema_to_json_schema, output_schema_to_json_schema,
};
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::unstructured::unstructured_or_raw_kind;
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
        JsonSchemaConfig::WITHOUT_DRAFT_MARKER,
    ))
}

/// Merge the constructor's user-supplied parameters with the method's and
/// render them as a single MCP JSON object schema. Auto-injected fields are
/// dropped by the renderer; `required` is computed option-aware.
///
/// When a user-supplied parameter name appears on both sides, the colliding
/// names are disambiguated (`constructor_<name>` / `method_<name>`) so the two
/// properties don't collapse into one. The same mapping is recomputed on the
/// invoke path to translate the advertised names back before extraction (see
/// [`field_name_mapping`]).
pub fn combined_input_schema(
    graph: &SchemaGraph,
    constructor: &AgentConstructorSchema,
    method: &AgentMethodSchema,
) -> JsonObject {
    let mapping = field_name_mapping(constructor, method);
    let mut fields: Vec<NamedField> =
        mapping.apply_to_constructor_fields(constructor.input_schema.fields());
    fields.extend(mapping.apply_to_method_fields(method.input_schema.fields()));
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
    if is_unstructured_output(graph, ty) || renders_as_resource_link(graph, ty) {
        return None;
    }
    let mut inner =
        output_schema_to_json_schema(graph, output, JsonSchemaConfig::WITHOUT_DRAFT_MARKER)?;
    // The rendered output is a self-contained JSON Schema document: any named
    // definitions live in a `$defs` object at *its* root and every `$ref`
    // points at `#/$defs/<id>` (document-root relative). Because we re-root the
    // value under `properties`, those `$defs` must be hoisted up to the wrapper
    // root, otherwise the refs would dangle. This mirrors
    // `input_schema_to_json_schema`, which keeps `$defs` at the document root.
    let defs = inner.as_object_mut().and_then(|obj| obj.remove("$defs"));
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
    let mut wrapper = json!({
        "type": "object",
        "properties": { FALLBACK_OUTPUT_FIELD_NAME: inner },
        "required": required,
    });
    if let Some(defs) = defs {
        wrapper
            .as_object_mut()
            .expect("wrapper is a JSON object literal")
            .insert("$defs".to_string(), defs);
    }
    Some(rmcp::model::object(wrapper))
}

/// Whether the output type is unstructured — a raw `Text` / `Binary` rich
/// scalar or a canonical unstructured `variant { inline, url }` wrapper — or
/// multimodal, in which case MCP omits the output schema and clients render the
/// content array instead.
fn is_unstructured_output(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    // Output refs are pre-validated in `from_agent_method` (via the legacy
    // projection of the method's output schema), so this classification runs on
    // fully-resolvable refs; the `unwrap_or(false)` / `Err` fallbacks only guard
    // truly unreachable cases rather than masking real ref errors.
    if is_multimodal_schema_type(graph, ty).unwrap_or(false) {
        return true;
    }
    unstructured_or_raw_kind(graph, ty)
        .map(|k| k.is_some())
        .unwrap_or(false)
}

fn resolves_to_option(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(graph.resolve_ref(ty), Ok(SchemaType::Option { .. }))
}

/// Whether a root output renders as an MCP `resource_link` content block rather
/// than structured content — a bare rich `Url` or `Path` scalar (see
/// `schema_value_to_tool_result`). MCP omits the output schema for these so
/// clients render the content array instead of validating structured content.
fn renders_as_resource_link(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(
        graph.resolve_ref(ty),
        Ok(SchemaType::Url { .. } | SchemaType::Path { .. })
    )
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
    fn colliding_constructor_and_method_params_are_disambiguated() {
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![NamedField::user_supplied("id", SchemaType::string())]);
        let meth = method(
            vec![NamedField::user_supplied("id", SchemaType::u32())],
            OutputSchema::Single(Box::new(SchemaType::string())),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        let props = schema.input_schema["properties"].as_object().unwrap();

        // The shared `id` name must not collapse: both sides are present under
        // disambiguated names, and the raw `id` is gone.
        assert!(
            !props.contains_key("id"),
            "raw collide name leaked: {props:#?}"
        );
        assert!(props.contains_key("constructor_id"));
        assert!(props.contains_key("method_id"));
        // Types are preserved per side (constructor = string, method = integer).
        assert_eq!(props["constructor_id"]["type"], json!("string"));
        assert_eq!(props["method_id"]["type"], json!("integer"));

        let required = schema.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("constructor_id")));
        assert!(required.contains(&json!("method_id")));
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
    fn url_output_has_no_schema() {
        use golem_common::schema::schema_type::UrlRestrictions;
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::url(UrlRestrictions::default()))),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        assert!(schema.output_schema.is_none());
    }

    #[test]
    fn path_output_has_no_schema() {
        use golem_common::schema::schema_type::{PathDirection, PathKind, PathSpec};
        let graph = SchemaGraph::empty();
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::path(PathSpec {
                direction: PathDirection::Output,
                kind: PathKind::File,
                allowed_mime_types: None,
                allowed_extensions: None,
            }))),
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

    #[test]
    fn structured_output_hoists_defs_to_root_so_refs_resolve() {
        use golem_common::schema::graph::{SchemaGraph as Graph, SchemaTypeDef};
        use golem_common::schema::metadata::TypeId;

        // An output type that references a named definition forces the
        // renderer to emit a `$defs` table plus a `$ref` into it. The wrapped
        // MCP output schema must keep `$defs` at the document root so the
        // `#/$defs/<id>` ref still resolves.
        let id = TypeId::new("MyType");
        let graph = Graph {
            defs: vec![SchemaTypeDef {
                id: id.clone(),
                name: None,
                body: SchemaType::bool(),
            }],
            root: SchemaType::bool(),
        };
        let ctor = constructor(vec![]);
        let meth = method(
            vec![NamedField::user_supplied("city", SchemaType::string())],
            OutputSchema::Single(Box::new(SchemaType::ref_to(id))),
        );
        let schema = get_mcp_tool_schema(&graph, &ctor, &meth);
        let out = schema.output_schema.expect("structured output schema");

        // `$defs` hoisted to the wrapper root, and the value is a `$ref`.
        assert!(
            out["$defs"].is_object(),
            "expected $defs at the wrapper root: {out:#?}"
        );
        let value_ref = out["properties"]["value"]["$ref"]
            .as_str()
            .expect("value property is a $ref");
        let key = value_ref
            .strip_prefix("#/$defs/")
            .expect("ref points into $defs");
        assert!(
            out["$defs"][key].is_object(),
            "ref {value_ref} must resolve against the wrapper-root $defs"
        );
    }

    use golem_common::schema::graph::SchemaTypeDef;
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::proptest_strategies::schema_graph_strategy;
    use golem_common::schema::schema_type::NamedFieldType;
    use proptest::prelude::*;

    /// Collect every local `$ref` in `doc` that does not resolve against the
    /// document root (RFC 6901 JSON Pointer semantics via `Value::pointer`).
    fn unresolved_local_refs(doc: &Value) -> Vec<String> {
        fn walk(doc: &Value, node: &Value, out: &mut Vec<String>) {
            match node {
                Value::Object(obj) => {
                    if let Some(Value::String(reference)) = obj.get("$ref") {
                        match reference.strip_prefix('#') {
                            Some(pointer) if doc.pointer(pointer).is_some() => {}
                            Some(_) => out.push(reference.clone()),
                            None => out.push(format!("non-local ref: {reference}")),
                        }
                    }
                    for child in obj.values() {
                        walk(doc, child, out);
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        walk(doc, child, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(doc, doc, &mut out);
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// The MCP input and output schemas produced by `get_mcp_tool_schema`
        /// must be self-contained: every `$ref` resolves against the document
        /// root. This guards the wrapping / re-rooting done by
        /// `structured_output_schema` and `combined_input_schema`. We force the
        /// output to be a named ref to a record so the structured-output path
        /// (which emits `$defs`) always runs.
        #[test]
        fn mcp_tool_schemas_are_self_contained(mut graph in schema_graph_strategy()) {
            let forced_id = TypeId::new("mcp/forced~output");
            let forced_body = SchemaType::record(vec![NamedFieldType {
                name: "payload".to_string(),
                body: graph.root.clone(),
                metadata: Default::default(),
            }]);
            graph.defs.push(SchemaTypeDef {
                id: forced_id.clone(),
                name: None,
                body: forced_body,
            });
            let ref_ty = SchemaType::ref_to(forced_id);

            let ctor = constructor(vec![NamedField::user_supplied(
                "ctor_arg",
                ref_ty.clone(),
            )]);
            let meth = method(
                vec![NamedField::user_supplied("arg", ref_ty.clone())],
                OutputSchema::Single(Box::new(ref_ty)),
            );
            let schema = get_mcp_tool_schema(&graph, &ctor, &meth);

            let input_doc = Value::Object(schema.input_schema);
            prop_assert!(
                input_doc["type"] == json!("object"),
                "input schema is not an object:\n{input_doc:#}"
            );
            let unresolved = unresolved_local_refs(&input_doc);
            prop_assert!(
                unresolved.is_empty(),
                "input schema has unresolved refs {unresolved:?}:\n{input_doc:#}"
            );

            let output_doc = Value::Object(
                schema
                    .output_schema
                    .expect("forced structured output schema"),
            );
            prop_assert!(
                output_doc["type"] == json!("object"),
                "output schema is not an object:\n{output_doc:#}"
            );
            prop_assert!(
                output_doc["properties"][FALLBACK_OUTPUT_FIELD_NAME]
                    .get("$defs")
                    .is_none(),
                "inner value schema must have had $defs hoisted to wrapper root:\n{output_doc:#}"
            );
            let unresolved = unresolved_local_refs(&output_doc);
            prop_assert!(
                unresolved.is_empty(),
                "output schema has unresolved refs {unresolved:?}:\n{output_doc:#}"
            );
        }
    }
}
