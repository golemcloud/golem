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
use serde_json::json;
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
    let schemas = bundle["components"]["schemas"]
        .as_object()
        .expect("schemas object");
    assert!(schemas.contains_key("union__branch__a"));
    assert!(schemas.contains_key("union__branch__b"));
    // Discriminator mapping must be rewritten to point at
    // `#/components/schemas/...`.
    let mapping = bundle["root"]["discriminator"]["mapping"]
        .as_object()
        .expect("discriminator mapping rewritten");
    assert_eq!(mapping["a"], json!("#/components/schemas/union__branch__a"));
    assert_eq!(mapping["b"], json!("#/components/schemas/union__branch__b"));
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
