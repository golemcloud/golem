// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

//! Thin JSON bridge between the schema model and the OpenAPI 3.1 document.
//!
//! All `SchemaType` rendering goes through the Wave-1 renderer
//! [`to_openapi_components`]; this module only splits the renderer's bundle
//! into the inline root schema and the document-wide `components/schemas`
//! entries, and provides the handful of fixed JSON schemas the emitter needs
//! for non-schema-bearing bodies/headers (binary bodies, enum'd strings, …).

use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render::to_openapi_components;
use golem_common::schema::schema_type::SchemaType;
use serde_json::{Map, Value, json};

/// Render `(graph, ty)` to an OpenAPI 3.1 schema JSON value, merging every
/// named component schema it references into the document-wide
/// `components/schemas` accumulator.
///
/// Returns the inline root schema for `ty` (a `{ "$ref": … }` object when `ty`
/// is a named `Ref`). Named definitions reachable from `ty` — including the
/// synthesised per-union-branch schemas — are merged into `components` under
/// their `TypeId` keys: an identical entry is deduplicated, a conflicting one
/// is an error. The document-wide [`SchemaGraphBuilder`] disambiguates names,
/// so a real conflict here indicates a bug.
///
/// [`SchemaGraphBuilder`]: golem_common::schema::adapters::SchemaGraphBuilder
pub fn render_schema(
    graph: &SchemaGraph,
    ty: &SchemaType,
    components: &mut Map<String, Value>,
) -> Result<Value, String> {
    let bundle = to_openapi_components(graph, ty);
    let Value::Object(mut bundle) = bundle else {
        return Err("OpenAPI renderer returned a non-object bundle".to_string());
    };

    if let Some(Value::Object(wrapper)) = bundle.get_mut("components")
        && let Some(Value::Object(schemas)) = wrapper.get_mut("schemas")
    {
        for (key, value) in std::mem::take(schemas) {
            match components.get(&key) {
                Some(existing) if existing == &value => {}
                Some(_) => {
                    return Err(format!(
                        "conflicting component schema for `{key}` while building OpenAPI document"
                    ));
                }
                None => {
                    components.insert(key, value);
                }
            }
        }
    }

    Ok(bundle
        .remove("root")
        .unwrap_or_else(|| Value::Object(Map::new())))
}

/// Plain `string` schema with no additional constraints.
pub fn string_schema() -> Value {
    json!({ "type": "string" })
}

/// `string` schema whose `enum` lists the given values. An empty list yields a
/// plain `string` schema (no `enum`).
pub fn string_enum_schema(values: &[String]) -> Value {
    if values.is_empty() {
        string_schema()
    } else {
        json!({ "type": "string", "enum": values })
    }
}

/// Arbitrary-binary body schema (`string` with `format: binary`), matching the
/// legacy emitter's opaque-binary placement.
pub fn arbitrary_binary_schema() -> Value {
    json!({ "type": "string", "format": "binary" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::schema_type::{NamedFieldType, SchemaType};
    use test_r::test;

    fn user_graph() -> (SchemaGraph, TypeId) {
        let id = TypeId::new("app.User");
        let def = SchemaTypeDef {
            id: id.clone(),
            name: Some("User".to_string()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "id".to_string(),
                body: SchemaType::u32(),
                metadata: Default::default(),
            }]),
        };
        (
            SchemaGraph {
                defs: vec![def],
                root: SchemaType::bool(),
            },
            id,
        )
    }

    #[test]
    fn scalar_produces_no_components() {
        let graph = SchemaGraph::anonymous(SchemaType::bool());
        let mut components = Map::new();
        let root = render_schema(&graph, &SchemaType::string(), &mut components).unwrap();
        assert_eq!(root, json!({ "type": "string" }));
        assert!(components.is_empty());
    }

    #[test]
    fn ref_emits_component_and_ref_root() {
        let (graph, id) = user_graph();
        let mut components = Map::new();
        let root = render_schema(&graph, &SchemaType::ref_to(id), &mut components).unwrap();
        assert_eq!(root["$ref"], json!("#/components/schemas/app.User"));
        assert!(components.contains_key("app.User"));
    }

    #[test]
    fn identical_component_is_deduplicated() {
        let (graph, id) = user_graph();
        let mut components = Map::new();
        render_schema(&graph, &SchemaType::ref_to(id.clone()), &mut components).unwrap();
        render_schema(&graph, &SchemaType::ref_to(id), &mut components).unwrap();
        assert_eq!(components.len(), 1);
    }

    #[test]
    fn conflicting_component_errors() {
        let (graph, id) = user_graph();
        let mut components = Map::new();
        components.insert("app.User".to_string(), json!({ "type": "string" }));
        let err = render_schema(&graph, &SchemaType::ref_to(id), &mut components).unwrap_err();
        assert!(
            err.contains("conflicting component schema"),
            "unexpected error: {err}"
        );
    }
}
