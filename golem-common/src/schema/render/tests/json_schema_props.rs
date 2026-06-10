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

//! Property-based tests asserting that every JSON Schema document produced by
//! the renderer is *self-contained*: each local `$ref` resolves, via RFC 6901
//! JSON Pointer semantics, against the document root. This is the invariant
//! that broke when a downstream consumer re-rooted a rendered document without
//! hoisting its `$defs` (see the MCP exporter regression).

use crate::schema::agent::{InputSchema, NamedField, OutputSchema};
use crate::schema::graph::SchemaTypeDef;
use crate::schema::metadata::TypeId;
use crate::schema::proptest_strategies::schema_graph_strategy;
use crate::schema::render::json_schema::{
    JsonSchemaConfig, input_schema_to_json_schema, output_schema_to_json_schema, to_json_schema,
};
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use proptest::prelude::*;
use serde_json::Value;
use test_r::test;

/// Collect every local `$ref` in `doc` that does NOT resolve against the
/// document root. A non-empty result means the document is not self-contained.
///
/// Uses [`serde_json::Value::pointer`], which already applies RFC 6901 token
/// unescaping (`~1` -> `/`, `~0` -> `~`), so this matches how a JSON Schema
/// validator resolves `#/$defs/<escaped-id>` pointers.
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

    /// Every document the renderer emits - canonical type document, MCP input
    /// schema, and MCP output schema - must be self-contained. We force a named
    /// definition whose id contains both `/` and `~` so the property always
    /// exercises `$defs` emission *and* JSON Pointer escaping.
    #[test]
    fn rendered_json_schema_documents_are_self_contained(
        mut graph in schema_graph_strategy(),
    ) {
        // A named type that wraps the generated root, referenced from every
        // rendered position below. The id deliberately contains pointer-escape
        // characters.
        let forced_id = TypeId::new("prop/forced~ref");
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

        let canonical = to_json_schema(&graph, &ref_ty);

        let input = InputSchema::Parameters(vec![NamedField::user_supplied(
            "arg",
            ref_ty.clone(),
        )]);
        let input_doc = input_schema_to_json_schema(&graph, &input, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);

        let output = OutputSchema::Single(Box::new(ref_ty));
        let output_doc = output_schema_to_json_schema(&graph, &output, JsonSchemaConfig::WITHOUT_DRAFT_MARKER)
            .expect("single output renders");

        for (label, doc) in [
            ("canonical", &canonical),
            ("mcp-input", &input_doc),
            ("mcp-output", &output_doc),
        ] {
            let unresolved = unresolved_local_refs(doc);
            prop_assert!(
                unresolved.is_empty(),
                "{label} schema has unresolved refs {unresolved:?}:\n{doc:#}"
            );
        }
    }
}
