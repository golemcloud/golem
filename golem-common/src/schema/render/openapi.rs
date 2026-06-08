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

use crate::schema::agent::{InputSchema, OutputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::render::json_schema::{
    JsonSchemaConfig, add_union_branch_defs, build_branch_name_table, input_schema_to_json_schema,
    output_schema_to_json_schema, render_defs, render_type,
};
use crate::schema::schema_type::SchemaType;
use serde_json::{Map, Value};

/// JSON Schema renderer configuration used for the OpenAPI input/output
/// bundles: canonical node shapes, but without the JSON Schema `$schema`
/// draft marker (OpenAPI does not accept it). Mirrors the configuration
/// implicitly used by [`to_openapi_components`], which renders the canonical
/// structural form and omits `$schema`.
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
    // `$schema` omission differ, both handled below. `OPENAPI_CONFIG` is the
    // shared policy for all three bundle functions.
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

/// Render an [`InputSchema`] to an OpenAPI 3.1 schema bundle.
///
/// Like [`to_openapi_components`], the returned object has shape
/// `{ "components": { "schemas": { … } }, "root": { … } }` and every `$ref`
/// pointer targets `#/components/schemas/<TypeId>`.
///
/// The `root` is an `object` schema whose `properties` are the input's
/// **user-supplied** parameters (`FieldSource::AutoInjected` fields are host
/// provided and never surfaced, so they are omitted) and whose `required`
/// array lists every user-supplied parameter whose schema is not an
/// `option<…>` — the exact behaviour of
/// [`input_schema_to_json_schema`](super::json_schema::input_schema_to_json_schema).
pub fn input_schema_to_openapi_components(graph: &SchemaGraph, input: &InputSchema) -> Value {
    let doc = input_schema_to_json_schema(graph, input, OPENAPI_CONFIG);
    json_schema_doc_to_openapi_bundle(doc)
}

/// Render an [`OutputSchema`] to an optional OpenAPI 3.1 schema bundle.
///
/// Returns `None` for [`OutputSchema::Unit`] (the method has no return
/// value). For [`OutputSchema::Single`] the bundle has the same shape as
/// [`to_openapi_components`].
///
/// This renderer applies no protocol policy (it does not, for example,
/// suppress multimodal outputs); the OpenAPI emitter decides response
/// shapes (status codes, content types) on top of the rendered schema.
pub fn output_schema_to_openapi_components(
    graph: &SchemaGraph,
    output: &OutputSchema,
) -> Option<Value> {
    let doc = output_schema_to_json_schema(graph, output, OPENAPI_CONFIG)?;
    Some(json_schema_doc_to_openapi_bundle(doc))
}

/// Transform a JSON Schema document (a root schema with an optional embedded
/// `$defs` map, as produced by
/// [`to_json_schema_with_config`](super::json_schema::to_json_schema_with_config))
/// into an OpenAPI bundle: pull `$defs` out into `components/schemas` and
/// rewrite every `#/$defs/…` pointer — in both `$ref`s and discriminator
/// mappings — to `#/components/schemas/…`.
///
/// `$defs` map keys are preserved raw (RFC 6901 §4); see the note in
/// [`to_openapi_components`] about OpenAPI-3.1 component-name constraints.
fn json_schema_doc_to_openapi_bundle(mut doc: Value) -> Value {
    let defs = if let Some(obj) = doc.as_object_mut() {
        // OpenAPI does not accept the JSON Schema `$schema` keyword; strip it
        // defensively in case a caller passed a config that emits it.
        obj.remove("$schema");
        match obj.remove("$defs") {
            Some(Value::Object(m)) => m,
            Some(_) => {
                debug_assert!(false, "JSON Schema renderer emitted non-object $defs");
                Map::new()
            }
            None => Map::new(),
        }
    } else {
        Map::new()
    };

    let root = rewrite_refs(doc);
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
