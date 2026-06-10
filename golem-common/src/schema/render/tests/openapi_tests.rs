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

use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::render::openapi::to_openapi_components;
use crate::schema::schema_type::{
    DiscriminatorRule, FieldDiscriminator, NamedFieldType, SchemaType, UnionBranch, UnionSpec,
};
use serde_json::{Value, json};
use test_r::test;

#[test]
fn refs_are_rewritten_to_components_schemas() {
    let user_id = TypeId::new("myapp.user");
    let user_def = SchemaTypeDef {
        id: user_id.clone(),
        name: Some("User".to_string()),
        body: SchemaType::record(vec![NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        }]),
    };
    let graph = SchemaGraph {
        defs: vec![user_def],
        root: SchemaType::ref_to(user_id.clone()),
    };
    let bundle = to_openapi_components(&graph, &graph.root);
    assert!(bundle["components"]["schemas"]["myapp.user"].is_object());
    assert_eq!(
        bundle["root"]["$ref"],
        json!("#/components/schemas/myapp.user")
    );
}

#[test]
fn root_schema_has_no_dollar_schema_keyword() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let bundle = to_openapi_components(&graph, &graph.root);
    assert!(bundle["root"].get("$schema").is_none());
}

/// Resolve a JSON Pointer (a string starting with `#`) against the
/// document root, performing RFC 6901 unescape of each token. Returns
/// `None` if the pointer fails to resolve.
fn resolve_local_ref<'a>(doc: &'a Value, reference: &str) -> Option<&'a Value> {
    let ptr = reference.strip_prefix('#')?;
    doc.pointer(ptr)
}

#[test]
fn openapi_type_id_with_slash_resolves_through_components_pointer() {
    // For a `TypeId` containing `/`, the JSON Schema `$defs` member name
    // is raw (per RFC 6901 §4) and `$ref` pointers carry the escaped form.
    // OpenAPI inherits the same property: `components.schemas` keys must be
    // raw, and the rewritten `#/components/schemas/<escaped>` pointer must
    // resolve via standard JSON Pointer rules (which unescape the token).
    let id = TypeId::new("ns/with/slash");
    let user_def = SchemaTypeDef {
        id: id.clone(),
        name: None,
        body: SchemaType::bool(),
    };
    let graph = SchemaGraph {
        defs: vec![user_def],
        root: SchemaType::ref_to(id),
    };
    let bundle = to_openapi_components(&graph, &graph.root);
    let schemas = bundle["components"]["schemas"]
        .as_object()
        .expect("schemas object");
    // The raw key must be present in `components.schemas`.
    assert!(
        schemas.contains_key("ns/with/slash"),
        "components.schemas should hold the raw key"
    );
    // And the rewritten `$ref` must resolve to it through actual JSON
    // Pointer semantics.
    let root_ref = bundle["root"]["$ref"].as_str().expect("root $ref");
    assert_eq!(root_ref, "#/components/schemas/ns~1with~1slash");
    assert!(
        resolve_local_ref(&bundle, root_ref).is_some(),
        "OpenAPI bundle must be JSON-Pointer-resolvable: ref {root_ref}"
    );
}

#[test]
fn openapi_type_id_with_tilde_resolves_through_components_pointer() {
    // Mirror of the slash case for `~`: raw `$defs` key, escaped `$ref`
    // pointer token; the rewritten OpenAPI pointer must still resolve.
    let id = TypeId::new("ns~with/slash");
    let user_def = SchemaTypeDef {
        id: id.clone(),
        name: None,
        body: SchemaType::bool(),
    };
    let graph = SchemaGraph {
        defs: vec![user_def],
        root: SchemaType::ref_to(id),
    };
    let bundle = to_openapi_components(&graph, &graph.root);
    let schemas = bundle["components"]["schemas"]
        .as_object()
        .expect("schemas object");
    assert!(schemas.contains_key("ns~with/slash"));
    let root_ref = bundle["root"]["$ref"].as_str().expect("root $ref");
    assert_eq!(root_ref, "#/components/schemas/ns~0with~1slash");
    assert!(
        resolve_local_ref(&bundle, root_ref).is_some(),
        "OpenAPI bundle must be JSON-Pointer-resolvable: ref {root_ref}"
    );
}

#[test]
fn openapi_binary_min_max_bytes_are_base64url_no_pad_encoded_lengths() {
    // Confirms the OpenAPI renderer reuses the JSON Schema binary helper
    // and therefore inherits the encoded-length conversion (raw `min_bytes`
    // / `max_bytes` -> base64url-no-pad character lengths).
    let restrictions = crate::schema::schema_type::BinaryRestrictions {
        min_bytes: Some(1),
        max_bytes: Some(4),
        ..Default::default()
    };
    let ty = SchemaType::binary(restrictions);
    let graph = SchemaGraph::anonymous(ty.clone());
    let bundle = to_openapi_components(&graph, &ty);
    let bytes = &bundle["root"]["properties"]["bytes"];
    assert_eq!(bytes["minLength"], json!(2));
    assert_eq!(bytes["maxLength"], json!(6));
}

#[test]
fn union_branch_defs_are_emitted_as_components() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "a".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("a".to_string()),
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "b".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("b".to_string()),
                }),
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let bundle = to_openapi_components(&graph, &ty);
    // Discriminator mapping must be rewritten to point at
    // `#/components/schemas/...`, and each rewritten pointer must
    // resolve through JSON Pointer semantics.
    let mapping = bundle["root"]["discriminator"]["mapping"]
        .as_object()
        .expect("discriminator mapping rewritten");
    let a_ref = mapping["a"].as_str().expect("a ref");
    let b_ref = mapping["b"].as_str().expect("b ref");
    assert_ne!(
        a_ref, b_ref,
        "branches with different bodies must not collide"
    );
    assert!(a_ref.starts_with("#/components/schemas/"));
    assert!(b_ref.starts_with("#/components/schemas/"));
    assert!(
        resolve_local_ref(&bundle, a_ref).is_some(),
        "branch a ref {a_ref} must resolve"
    );
    assert!(
        resolve_local_ref(&bundle, b_ref).is_some(),
        "branch b ref {b_ref} must resolve"
    );
}

#[test]
fn nested_refs_inside_schemas_are_rewritten() {
    let user_id = TypeId::new("u");
    let group_id = TypeId::new("g");
    let user_def = SchemaTypeDef {
        id: user_id.clone(),
        name: None,
        body: SchemaType::u32(),
    };
    let group_def = SchemaTypeDef {
        id: group_id.clone(),
        name: None,
        body: SchemaType::list(SchemaType::ref_to(user_id.clone())),
    };
    let graph = SchemaGraph {
        defs: vec![user_def, group_def.clone()],
        root: SchemaType::ref_to(group_id),
    };
    let bundle = to_openapi_components(&graph, &graph.root);
    let group_schema = &bundle["components"]["schemas"]["g"];
    assert_eq!(
        group_schema["items"]["$ref"],
        json!("#/components/schemas/u")
    );
}
