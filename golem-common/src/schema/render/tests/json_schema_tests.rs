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

use crate::schema::graph::SchemaGraph;
use crate::schema::render::json_schema::to_json_schema;
use crate::schema::schema_type::{
    DiscriminatorRule, FieldDiscriminator, NamedFieldType, SchemaType, TextRestrictions,
    UnionBranch, UnionSpec,
};
use serde_json::{Value, json};
use test_r::test;

#[test]
fn record_emits_object_with_properties() {
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "name".to_string(),
            body: SchemaType::text(TextRestrictions {
                min_length: Some(1),
                max_length: Some(64),
                ..Default::default()
            }),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("object"));
    let props = schema["properties"]
        .as_object()
        .expect("properties is object");
    assert!(props.contains_key("id"));
    assert!(props.contains_key("name"));
    let required = schema["required"].as_array().expect("required is array");
    assert!(required.contains(&Value::String("id".to_string())));
    assert!(required.contains(&Value::String("name".to_string())));
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn primitive_integer_carries_min_max() {
    let ty = SchemaType::s8();
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("integer"));
    assert_eq!(schema["minimum"], json!(i8::MIN as i64));
    assert_eq!(schema["maximum"], json!(i8::MAX as i64));
}

#[test]
fn list_emits_array_with_items() {
    let ty = SchemaType::list(SchemaType::string());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("array"));
    assert_eq!(schema["items"]["type"], json!("string"));
}

#[test]
fn fixed_list_emits_min_max() {
    let ty = SchemaType::fixed_list(SchemaType::bool(), 3);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["minItems"], json!(3));
    assert_eq!(schema["maxItems"], json!(3));
}

#[test]
fn map_emits_array_of_pairs() {
    let ty = SchemaType::map(SchemaType::u32(), SchemaType::string());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("array"));
    assert_eq!(schema["items"]["type"], json!("array"));
    let prefix = schema["items"]["prefixItems"]
        .as_array()
        .expect("prefixItems is array");
    assert_eq!(prefix.len(), 2);
}

#[test]
fn option_emits_one_of_null_and_inner() {
    let ty = SchemaType::option(SchemaType::bool());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    let one_of = schema["oneOf"].as_array().expect("oneOf is array");
    assert_eq!(one_of.len(), 2);
}

#[test]
fn variant_emits_one_of_with_const_or_object() {
    let ty = SchemaType::variant(vec![
        crate::schema::schema_type::VariantCaseType {
            name: "ready".to_string(),
            payload: None,
            metadata: Default::default(),
        },
        crate::schema::schema_type::VariantCaseType {
            name: "value".to_string(),
            payload: Some(SchemaType::u32()),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    let one_of = schema["oneOf"].as_array().expect("oneOf is array");
    assert_eq!(one_of.len(), 2);
    assert_eq!(one_of[0]["const"], json!("ready"));
    assert_eq!(one_of[1]["type"], json!("object"));
}

#[test]
fn union_with_field_equals_emits_discriminator() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "left".to_string(),
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
                tag: "right".to_string(),
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
    let schema = to_json_schema(&graph, &ty);
    assert!(
        schema.get("discriminator").is_some(),
        "expected discriminator"
    );
    assert_eq!(schema["discriminator"]["propertyName"], json!("kind"));
}

#[test]
fn enum_emits_string_enum() {
    let ty = SchemaType::r#enum(vec!["red".into(), "green".into(), "blue".into()]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("string"));
    let cases = schema["enum"].as_array().expect("enum is array");
    assert_eq!(cases.len(), 3);
}

#[test]
fn flags_emits_array_of_string_enum() {
    let ty = SchemaType::flags(vec!["a".into(), "b".into()]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("array"));
    assert_eq!(schema["uniqueItems"], json!(true));
}

#[test]
fn secret_emits_canonical_object_shape() {
    let ty = SchemaType::secret(crate::schema::schema_type::SecretSpec::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["properties"]["secret_ref"]["type"], json!("string"));
    assert_eq!(schema["properties"]["secret_ref"]["minLength"], json!(1));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&Value::String("secret_ref".to_string()))
    );
}

#[test]
fn datetime_emits_date_time_format() {
    let ty = SchemaType::datetime();
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("string"));
    assert_eq!(schema["format"], json!("date-time"));
}

#[test]
fn root_schema_carries_draft_2020_12_marker() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let schema = to_json_schema(&graph, &graph.root);
    assert_eq!(
        schema["$schema"],
        json!("https://json-schema.org/draft/2020-12/schema")
    );
}

#[test]
fn tuple_carries_min_items_equal_to_arity() {
    let ty = SchemaType::tuple(vec![
        SchemaType::u32(),
        SchemaType::string(),
        SchemaType::bool(),
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["minItems"], json!(3));
    assert_eq!(schema["items"], json!(false));
}

#[test]
fn type_id_with_slash_is_pointer_escaped() {
    use crate::schema::graph::SchemaTypeDef;
    use crate::schema::metadata::TypeId;
    let id = TypeId::new("ns/with/slash");
    let graph = crate::schema::graph::SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::bool(),
        }],
        root: SchemaType::ref_to(id),
    };
    let schema = to_json_schema(&graph, &graph.root);
    // The root schema is the $ref pointer; slashes inside the token must
    // be escaped per RFC 6901.
    assert_eq!(schema["$ref"], json!("#/$defs/ns~1with~1slash"));
    assert!(schema["$defs"]["ns~1with~1slash"].is_object());
}

#[test]
fn binary_emits_canonical_object_shape() {
    let ty = SchemaType::binary(crate::schema::schema_type::BinaryRestrictions::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["properties"]["bytes"]["type"], json!("string"));
    assert_eq!(
        schema["properties"]["bytes"]["contentEncoding"],
        json!("base64url")
    );
    assert_eq!(schema["properties"]["mime_type"]["type"], json!("string"));
    assert_eq!(
        schema["properties"]["mime_type"]["pattern"]
            .as_str()
            .unwrap(),
        "^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$"
    );
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn quota_token_emits_canonical_object_shape() {
    let ty = SchemaType::quota_token(crate::schema::schema_type::QuotaTokenSpec::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(
        schema["properties"]["environment_id"]["format"],
        json!("uuid")
    );
    assert_eq!(
        schema["properties"]["last_credit_at"]["format"],
        json!("date-time")
    );
    let expected = schema["properties"]["expected_use"]["oneOf"]
        .as_array()
        .unwrap();
    assert_eq!(expected.len(), 2);
    let req: std::collections::HashSet<String> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    for f in [
        "environment_id",
        "resource_name",
        "expected_use",
        "last_credit",
        "last_credit_at",
    ] {
        assert!(req.contains(f));
    }
}

#[test]
fn union_emits_per_branch_defs() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "left".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("L".to_string()),
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "right".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("R".to_string()),
                }),
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    // Per-branch defs must exist and the mapping must point at them.
    assert!(schema["$defs"]["union__branch__left"].is_object());
    assert!(schema["$defs"]["union__branch__right"].is_object());
    let mapping = schema["discriminator"]["mapping"]
        .as_object()
        .expect("discriminator mapping");
    assert_eq!(mapping["L"], json!("#/$defs/union__branch__left"));
    assert_eq!(mapping["R"], json!("#/$defs/union__branch__right"));
    // The branch def must carry the discriminator constraint (`const` on
    // the field).
    assert_eq!(
        schema["$defs"]["union__branch__left"]["properties"]["kind"]["const"],
        json!("L")
    );
}
