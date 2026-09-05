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

//! Thin OpenAPI 3.1 adaptor over [`super::json_schema`].
//!
//! Re-emits the same per-node shape but reroutes JSON-Schema's `$defs`
//! into OpenAPI's `components/schemas` (and rewrites the corresponding
//! `$ref` pointers). Per-branch union schemas (synthesised by
//! [`super::json_schema::add_union_branch_defs`] under tag-derived keys
//! resolved via [`super::json_schema::BranchNameTable`]) ship as
//! additional component schemas so discriminator mappings resolve.

use crate::schema::graph::SchemaGraph;
use crate::schema::render::json_schema::{
    JsonSchemaConfig, add_union_branch_defs, build_branch_name_table, render_defs, render_type,
};
use crate::schema::schema_type::SchemaType;
use serde_json::{Map, Value};

/// JSON Schema renderer configuration used by [`to_openapi_components`]:
/// canonical node shapes, but without the JSON Schema `$schema` draft marker
/// (OpenAPI does not accept it).
const OPENAPI_CONFIG: JsonSchemaConfig = JsonSchemaConfig {
    include_draft_marker: false,
};

/// Render `(graph, ty)` to an OpenAPI 3.1 schema bundle.
///
/// The returned object has shape:
///
/// ```json
/// {
///   "components": { "schemas": { "<TypeId>": {…}, … } },
///   "root": { … the root schema … }
/// }
/// ```
///
/// `$ref` pointers inside the schemas point at
/// `#/components/schemas/<TypeId>` (rewritten from `#/$defs/<TypeId>`).
/// OpenAPI does not accept the JSON Schema `$schema` keyword, so it is
/// never emitted here.
pub fn to_openapi_components(graph: &SchemaGraph, ty: &SchemaType) -> Value {
    // OpenAPI renders the canonical structural form; only ref-rewriting and
    // `$schema` omission differ, both handled below.
    let config = OPENAPI_CONFIG;
    let table = build_branch_name_table(graph, ty);
    let root = rewrite_refs(render_type(graph, ty, true, &table, config));
    let mut defs = render_defs(graph, &table, config);
    add_union_branch_defs(graph, ty, &mut defs, &table, config);
    // `$defs` map keys are raw per RFC 6901 §4: the JSON Pointer token in a
    // `$ref` is the *escaped* form of the resolved object member name.
    // We preserve raw keys when moving `$defs` → `components.schemas` so
    // that an escaped `#/components/schemas/<token>` pointer resolves
    // back to the raw key the same way it did under `$defs`.
    //
    // Note: this is JSON-Pointer-correct, but OpenAPI 3.1 additionally
    // constrains component-name characters to `[A-Za-z0-9._-]`. TypeIds
    // containing `/`, `~`, or other characters outside that set will
    // produce technically-invalid component names. Adopting an
    // OpenAPI-safe component-name registry is tracked as a follow-up;
    // typical TypeIds never trigger this.
    let schemas: Map<String, Value> = defs
        .into_iter()
        .map(|(k, v)| (k, rewrite_refs(v)))
        .collect();

    let mut out = Map::new();
    out.insert(
        "components".to_string(),
        Value::Object({
            let mut wrapper = Map::new();
            wrapper.insert("schemas".to_string(), Value::Object(schemas));
            wrapper
        }),
    );
    out.insert("root".to_string(), root);
    Value::Object(out)
}

fn rewrite_refs(mut v: Value) -> Value {
    rewrite_refs_in_place(&mut v);
    v
}

fn rewrite_refs_in_place(v: &mut Value) {
    match v {
        Value::Object(map) => {
            // Rewrite both `$ref` pointers and discriminator-mapping
            // targets.
            if let Some(Value::String(ptr)) = map.get_mut("$ref")
                && let Some(rest) = ptr.strip_prefix("#/$defs/")
            {
                *ptr = format!("#/components/schemas/{rest}");
            }
            if let Some(Value::Object(disc)) = map.get_mut("discriminator")
                && let Some(Value::Object(mapping)) = disc.get_mut("mapping")
            {
                for value in mapping.values_mut() {
                    if let Value::String(s) = value
                        && let Some(rest) = s.strip_prefix("#/$defs/")
                    {
                        *s = format!("#/components/schemas/{rest}");
                    }
                }
            }
            for value in map.values_mut() {
                rewrite_refs_in_place(value);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                rewrite_refs_in_place(item);
            }
        }
        _ => {}
    }
}
