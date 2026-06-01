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
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::render::docs::graph_to_markdown;
use crate::schema::schema_type::{
    DiscriminatorRule, FieldDiscriminator, NamedFieldType, SchemaType, TextRestrictions,
    UnionBranch, UnionSpec,
};
use test_r::test;

#[test]
fn graph_to_markdown_includes_root_and_defs() {
    let user_id = TypeId::new("User");
    let user_body = SchemaType::record(vec![
        NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u32(),
            metadata: MetadataEnvelope {
                doc: Some("Stable identifier".to_string()),
                ..Default::default()
            },
        },
        NamedFieldType {
            name: "name".to_string(),
            body: SchemaType::text(TextRestrictions::default()),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: user_id.clone(),
            name: Some("User".to_string()),
            body: user_body.with_metadata(MetadataEnvelope {
                doc: Some("A user record.".to_string()),
                examples: vec![r#"{ "id": 1, "name": { "text": "Ada" } }"#.to_string()],
                ..Default::default()
            }),
        }],
        root: SchemaType::ref_to(user_id),
    };
    let md = graph_to_markdown(&graph, "Root");
    assert!(md.contains("## Root"));
    assert!(md.contains("## User"));
    assert!(md.contains("A user record."));
    assert!(md.contains("Stable identifier"));
    assert!(md.contains("### Examples"));
    assert!(md.contains("```json"));
    assert!(md.contains("### Fields"));
}

#[test]
fn root_ref_inlines_referenced_body_fields() {
    // The root is `Ref(User)`; the rendered root section must show the
    // record's fields directly, not just the ref pointer.
    let user_id = TypeId::new("user.Profile");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: user_id.clone(),
            name: Some("Profile".to_string()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "handle".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            }]),
        }],
        root: SchemaType::ref_to(user_id),
    };
    let md = graph_to_markdown(&graph, "InboundProfile");
    assert!(md.contains("## InboundProfile"));
    // The fields list must appear *under* the root section, not just
    // under the `## Profile` def section.
    let root_idx = md.find("## InboundProfile").unwrap();
    let user_idx = md.find("## Profile").unwrap();
    let fields_idx = md.find("### Fields").unwrap();
    assert!(fields_idx > root_idx && fields_idx < user_idx);
}

#[test]
fn union_renders_branches_subsection() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "ssh".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "ssh://".to_string(),
                },
                metadata: MetadataEnvelope {
                    doc: Some("Secure shell URL".to_string()),
                    examples: vec!["ssh://server".to_string()],
                    ..Default::default()
                },
            },
            UnionBranch {
                tag: "by_kind".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("custom".to_string()),
                }),
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty);
    let md = graph_to_markdown(&graph, "Source");
    assert!(md.contains("### Branches"));
    assert!(md.contains("`ssh`"));
    assert!(md.contains("`by_kind`"));
    // Branch-level doc and examples surface inline.
    assert!(md.contains("Secure shell URL"));
    assert!(md.contains("ssh://server"));
}
