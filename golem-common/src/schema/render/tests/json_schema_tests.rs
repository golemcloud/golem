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
    UnionBranch, UnionSpec, VariantCaseType,
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
fn record_option_field_is_not_required() {
    // An `option<…>` record field is present as a property but excluded from
    // `required`: it may be omitted, and an explicit `null` is still accepted
    // by the option's `oneOf [null, T]` schema.
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "nickname".to_string(),
            body: SchemaType::option(SchemaType::string()),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    let props = schema["properties"]
        .as_object()
        .expect("properties is object");
    assert!(props.contains_key("id"));
    assert!(props.contains_key("nickname"));
    let required = schema["required"].as_array().expect("required is array");
    assert_eq!(required, &vec![Value::String("id".to_string())]);
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
    assert_eq!(schema["properties"]["secretRef"]["type"], json!("string"));
    assert_eq!(schema["properties"]["secretRef"]["minLength"], json!(1));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&Value::String("secretRef".to_string()))
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

/// Resolve a JSON Pointer-style `#/...` reference against the document
/// root. Returns `None` if the pointer fails to resolve.
fn resolve_local_ref<'a>(doc: &'a Value, reference: &str) -> Option<&'a Value> {
    let ptr = reference.strip_prefix('#')?;
    doc.pointer(ptr)
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
    // Per RFC 6901 §4: the `$ref` pointer escapes `/` and `~` (`~1` /
    // `~0`), but the resolved `$defs` member name is the **raw** key.
    assert_eq!(schema["$ref"], json!("#/$defs/ns~1with~1slash"));
    assert!(schema["$defs"]["ns/with/slash"].is_object());
    assert!(schema["$defs"]["ns~1with~1slash"].is_null());
    // The `$ref` must actually resolve via JSON Pointer rules (which
    // unescape `~1` → `/`).
    let r = schema["$ref"].as_str().unwrap();
    assert!(
        resolve_local_ref(&schema, r).is_some(),
        "$ref {r} must resolve through JSON Pointer semantics"
    );
}

#[test]
fn type_id_with_tilde_is_pointer_escaped() {
    use crate::schema::graph::SchemaTypeDef;
    use crate::schema::metadata::TypeId;
    // `~` and `/` must both be escaped (`~0` / `~1`) per RFC 6901. The
    // pointer must round-trip back to the raw key on resolution.
    let id = TypeId::new("ns~with/slash");
    let graph = crate::schema::graph::SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::bool(),
        }],
        root: SchemaType::ref_to(id),
    };
    let schema = to_json_schema(&graph, &graph.root);
    assert_eq!(schema["$ref"], json!("#/$defs/ns~0with~1slash"));
    assert!(schema["$defs"]["ns~with/slash"].is_object());
    let r = schema["$ref"].as_str().unwrap();
    assert!(
        resolve_local_ref(&schema, r).is_some(),
        "$ref {r} must resolve through JSON Pointer semantics"
    );
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
    assert_eq!(schema["properties"]["mimeType"]["type"], json!("string"));
    assert_eq!(
        schema["properties"]["mimeType"]["pattern"]
            .as_str()
            .unwrap(),
        "^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$"
    );
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn binary_min_max_bytes_are_converted_to_base64url_no_pad_encoded_length() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    // For every n in [0, 6], the JSON `minLength` / `maxLength` must equal
    // the length of `URL_SAFE_NO_PAD.encode(vec![0; n])`. This pins the
    // encoded-length conversion to the canonical encoder.
    for n in 0u32..=6 {
        let expected = URL_SAFE_NO_PAD.encode(vec![0u8; n as usize]).len();
        let restrictions = crate::schema::schema_type::BinaryRestrictions {
            min_bytes: Some(n),
            max_bytes: Some(n),
            ..Default::default()
        };
        let ty = SchemaType::binary(restrictions);
        let graph = SchemaGraph::anonymous(ty.clone());
        let schema = to_json_schema(&graph, &ty);
        assert_eq!(
            schema["properties"]["bytes"]["minLength"],
            json!(expected),
            "n = {n}"
        );
        assert_eq!(
            schema["properties"]["bytes"]["maxLength"],
            json!(expected),
            "n = {n}"
        );
    }
}

#[test]
fn quota_token_emits_canonical_object_shape() {
    let ty = SchemaType::quota_token(crate::schema::schema_type::QuotaTokenSpec::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(
        schema["properties"]["environmentId"]["format"],
        json!("uuid")
    );
    assert_eq!(
        schema["properties"]["lastCreditAt"]["format"],
        json!("date-time")
    );
    let expected = schema["properties"]["expectedUse"]["oneOf"]
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
        "environmentId",
        "resourceName",
        "expectedUse",
        "lastCredit",
        "lastCreditAt",
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
    let defs = schema["$defs"].as_object().expect("$defs object");
    // Two distinct per-branch defs must exist; their keys are content-
    // hash-derived, so look them up by following the discriminator
    // mapping rather than asserting a specific string.
    let mapping = schema["discriminator"]["mapping"]
        .as_object()
        .expect("discriminator mapping");
    let left_ref = mapping["L"].as_str().expect("L ref");
    let right_ref = mapping["R"].as_str().expect("R ref");
    assert_ne!(left_ref, right_ref, "branch refs must differ");
    let left_key = left_ref
        .strip_prefix("#/$defs/")
        .expect("L ref points into $defs");
    let right_key = right_ref
        .strip_prefix("#/$defs/")
        .expect("R ref points into $defs");
    assert!(defs.contains_key(left_key));
    assert!(defs.contains_key(right_key));
    // The branch def must carry the discriminator constraint (`const` on
    // the field).
    assert_eq!(defs[left_key]["properties"]["kind"]["const"], json!("L"));
    assert_eq!(defs[right_key]["properties"]["kind"]["const"], json!("R"));
}

#[test]
fn unions_sharing_branch_tag_do_not_collide_in_defs() {
    // Two distinct unions inside the same root record both carry a
    // branch tag `"shared"` but with different bodies / discriminators.
    // The renderer must produce two distinct `$defs` entries (one per
    // union branch), not one collapsed entry.
    let union_a = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "shared".to_string(),
            body: SchemaType::record(vec![NamedFieldType {
                name: "kind".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            }]),
            discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: "kind".to_string(),
                literal: Some("A".to_string()),
            }),
            metadata: Default::default(),
        }],
    });
    let union_b = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "shared".to_string(),
            body: SchemaType::record(vec![NamedFieldType {
                name: "kind".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            }]),
            discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: "kind".to_string(),
                literal: Some("B".to_string()),
            }),
            metadata: Default::default(),
        }],
    });
    let root = SchemaType::record(vec![
        NamedFieldType {
            name: "a".to_string(),
            body: union_a,
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "b".to_string(),
            body: union_b,
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(root.clone());
    let schema = to_json_schema(&graph, &root);
    let defs = schema["$defs"].as_object().expect("$defs object");
    // Exactly two synthesised branch defs, one per union, even though
    // both branches share the tag `"shared"`. The anonymous graph has
    // no named defs, so every `$defs` entry is a synthesised branch.
    let branch_keys: Vec<&String> = defs.keys().collect();
    assert_eq!(
        branch_keys.len(),
        2,
        "expected two distinct branch defs, got {branch_keys:?}"
    );
    // Branch names should preserve the tag (sanitised) and disambiguate
    // by the parent record field, not fall back to opaque hashes.
    let names: std::collections::HashSet<&str> = branch_keys.iter().map(|k| k.as_str()).collect();
    assert!(
        names.iter().all(|n| !n.contains("_hash")),
        "branch names should not fall back to hash suffix: {names:?}"
    );
    // One def must carry `const = "A"`, the other `const = "B"`.
    let consts: std::collections::HashSet<&str> = branch_keys
        .iter()
        .filter_map(|k| defs[*k]["properties"]["kind"]["const"].as_str())
        .collect();
    assert!(consts.contains("A"), "missing branch A: {consts:?}");
    assert!(consts.contains("B"), "missing branch B: {consts:?}");
}

#[test]
fn unions_with_structurally_identical_branches_dedupe() {
    // Two unions whose branches are byte-for-byte identical may share
    // their `$defs` entry — the content-hash key collapses to a single
    // key, which is semantically correct (the schemas are the same).
    let branch = UnionBranch {
        tag: "x".to_string(),
        body: SchemaType::string(),
        discriminator: DiscriminatorRule::Prefix {
            prefix: "x:".to_string(),
        },
        metadata: Default::default(),
    };
    let union = SchemaType::union(UnionSpec {
        branches: vec![branch.clone()],
    });
    let root = SchemaType::record(vec![
        NamedFieldType {
            name: "a".to_string(),
            body: union.clone(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "b".to_string(),
            body: union,
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(root.clone());
    let schema = to_json_schema(&graph, &root);
    let defs = schema["$defs"].as_object().expect("$defs object");
    // Anonymous graph: every `$defs` entry is a synthesised branch def.
    let branch_keys: Vec<&String> = defs.keys().collect();
    assert_eq!(
        branch_keys.len(),
        1,
        "identical branches dedupe to a single def, got {branch_keys:?}"
    );
}

#[test]
fn multimodal_variant_canonical_renders_inline_tagged_oneof() {
    // Multimodal is modelled as a tagged `variant` with `Role::Multimodal`.
    // Under the canonical config it renders like any other variant: an
    // inline `oneOf` of `{ <case>: <payload> }` objects, with no `$defs`
    // indirection and no OpenAPI `discriminator` block.
    let mut variant = SchemaType::variant(vec![
        VariantCaseType {
            name: "caption".to_string(),
            payload: Some(SchemaType::string()),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "image_url".to_string(),
            payload: Some(SchemaType::string()),
            metadata: Default::default(),
        },
    ]);
    variant.metadata_mut().role = Some(crate::schema::metadata::Role::Multimodal);
    let graph = SchemaGraph::anonymous(variant.clone());
    let schema = to_json_schema(&graph, &variant);

    assert!(
        schema.get("discriminator").is_none(),
        "multimodal variant must not emit a `discriminator` block: {schema}"
    );
    let one_of = schema["oneOf"]
        .as_array()
        .expect("multimodal variant renders as `oneOf`");
    assert_eq!(one_of.len(), 2);
    // Each entry is the inline tagged-object shape; no `$ref`.
    for (entry, name) in one_of.iter().zip(["caption", "image_url"]) {
        assert!(
            entry.get("$ref").is_none(),
            "canonical multimodal variant entries are inline, not $ref: {entry}"
        );
        assert_eq!(entry["required"], json!([name]));
        assert_eq!(entry["properties"][name]["type"], json!("string"));
    }
}

#[test]
fn multimodal_variant_does_not_pollute_union_defs() {
    // A multimodal variant renders inline, so it must not contribute any
    // `$defs` entries; a sibling normal union still gets its branch defs.
    let mut multimodal_variant = SchemaType::variant(vec![VariantCaseType {
        name: "caption".to_string(),
        payload: Some(SchemaType::string()),
        metadata: Default::default(),
    }]);
    multimodal_variant.metadata_mut().role = Some(crate::schema::metadata::Role::Multimodal);
    let normal_union = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
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
        }],
    });
    let root = SchemaType::record(vec![
        NamedFieldType {
            name: "normal".to_string(),
            body: normal_union,
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "multimodal".to_string(),
            body: SchemaType::list(multimodal_variant),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(root.clone());
    let schema = to_json_schema(&graph, &root);

    // The normal union still resolves through `$defs` via `$ref`.
    let normal_ref = schema["properties"]["normal"]["oneOf"][0]["$ref"]
        .as_str()
        .expect("normal union renders via $ref");
    let normal_key = normal_ref.strip_prefix("#/$defs/").unwrap();
    let defs = schema["$defs"].as_object().expect("$defs object");
    assert_eq!(
        defs[normal_key]["properties"]["kind"]["const"],
        json!("L"),
        "normal branch def must carry the discriminator constraint"
    );
    // The multimodal variant list renders inline (array of inline `oneOf`).
    let item = &schema["properties"]["multimodal"]["items"];
    let one_of = item["oneOf"]
        .as_array()
        .expect("multimodal items are `oneOf`");
    assert!(
        one_of[0].get("$ref").is_none(),
        "multimodal variant entries are inline, not $ref: {item}"
    );
}

#[test]
fn union_branch_def_keys_preserve_tag() {
    // A single, unambiguous tag must produce a `$defs` key derived from
    // the tag (sanitised to UpperCamelCase), not an opaque content hash
    // — generated OpenAPI clients depend on this for readable type names.
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "image_url".to_string(),
            body: SchemaType::record(vec![NamedFieldType {
                name: "kind".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            }]),
            discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: "kind".to_string(),
                literal: Some("image_url".to_string()),
            }),
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let schema = to_json_schema(&graph, &ty);
    let defs = schema["$defs"].as_object().expect("$defs object");
    assert!(
        defs.contains_key("ImageUrl"),
        "expected `ImageUrl` key derived from tag `image_url`, got {:?}",
        defs.keys().collect::<Vec<_>>()
    );
    let one_of_ref = schema["oneOf"][0]["$ref"]
        .as_str()
        .expect("oneOf entry is a $ref");
    assert_eq!(one_of_ref, "#/$defs/ImageUrl");
}

#[test]
fn colliding_branch_tags_disambiguate_by_parent_field() {
    // Two unions inside the same root record both have a branch tagged
    // `shared`. The renderer must lift the parent record-field name into
    // the assigned name on both members (symmetric disambiguation).
    let make_union = |literal: &str| {
        SchemaType::union(UnionSpec {
            branches: vec![UnionBranch {
                tag: "shared".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some(literal.to_string()),
                }),
                metadata: Default::default(),
            }],
        })
    };
    let root = SchemaType::record(vec![
        NamedFieldType {
            name: "alpha".to_string(),
            body: make_union("A"),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "beta".to_string(),
            body: make_union("B"),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(root.clone());
    let schema = to_json_schema(&graph, &root);
    let defs = schema["$defs"].as_object().expect("$defs object");
    let keys: std::collections::HashSet<&str> = defs.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["AlphaShared", "BetaShared"].into_iter().collect(),
        "colliding tags must be disambiguated by the parent record field, got {keys:?}"
    );
}

// --------------------------------------------------------------------------
// Configurable renderer + agent-level (InputSchema/OutputSchema) entry points
// --------------------------------------------------------------------------

mod agent_entry_points {
    use super::*;
    use crate::schema::agent::{
        AutoInjectedKind, FieldSource, InputSchema, NamedField, OutputSchema,
    };
    use crate::schema::metadata::Role;
    use crate::schema::render::json_schema::{
        JsonSchemaConfig, input_schema_to_json_schema, output_schema_to_json_schema,
        to_json_schema_with_config,
    };
    use test_r::test;

    #[test]
    fn without_draft_marker_config_omits_draft_marker() {
        let ty = SchemaType::record(vec![NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        }]);
        let graph = SchemaGraph::anonymous(ty.clone());
        let canonical = to_json_schema_with_config(&graph, &ty, JsonSchemaConfig::CANONICAL);
        let without_marker =
            to_json_schema_with_config(&graph, &ty, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);
        assert!(canonical.get("$schema").is_some());
        assert!(
            without_marker.get("$schema").is_none(),
            "WITHOUT_DRAFT_MARKER config must omit the $schema draft marker: {without_marker}"
        );
    }

    #[test]
    fn input_schema_omits_auto_injected_and_marks_options_optional() {
        let input = InputSchema::Parameters(vec![
            NamedField::user_supplied("city", SchemaType::string()),
            NamedField::user_supplied("hint", SchemaType::option(SchemaType::string())),
            NamedField::auto_injected(
                "principal",
                AutoInjectedKind::Principal,
                SchemaType::string(),
            ),
        ]);
        let graph = SchemaGraph::empty();
        let doc =
            input_schema_to_json_schema(&graph, &input, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);
        assert_eq!(doc["type"], json!("object"));
        assert_eq!(doc["additionalProperties"], json!(false));
        let props = doc["properties"].as_object().expect("properties object");
        assert!(props.contains_key("city"));
        assert!(props.contains_key("hint"));
        assert!(
            !props.contains_key("principal"),
            "auto-injected fields must not be surfaced: {doc}"
        );
        let required = doc["required"].as_array().expect("required array");
        assert!(required.contains(&Value::String("city".to_string())));
        assert!(
            !required.contains(&Value::String("hint".to_string())),
            "option fields must not be required: {doc}"
        );
        assert!(
            !required.contains(&Value::String("principal".to_string())),
            "auto-injected fields must not be required: {doc}"
        );
        assert!(doc.get("$schema").is_none());
    }

    #[test]
    fn input_schema_attaches_defs_at_root() {
        // A record field type becomes a named def in the graph; the rendered
        // input schema must carry the $defs at its root and reference it.
        let user_ty = SchemaType::Ref {
            id: crate::schema::metadata::TypeId("myapp.user".to_string()),
            metadata: Default::default(),
        };
        let user_def_body = SchemaType::record(vec![NamedFieldType {
            name: "name".to_string(),
            body: SchemaType::string(),
            metadata: Default::default(),
        }]);
        let mut graph = SchemaGraph::empty();
        graph.defs.push(crate::schema::graph::SchemaTypeDef {
            id: crate::schema::metadata::TypeId("myapp.user".to_string()),
            name: Some("User".to_string()),
            body: user_def_body,
        });
        let input = InputSchema::Parameters(vec![NamedField::user_supplied("user", user_ty)]);
        let doc =
            input_schema_to_json_schema(&graph, &input, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);
        assert!(
            doc.get("$defs").and_then(|d| d.get("myapp.user")).is_some(),
            "named def must be attached at the document root: {doc}"
        );
        let user_prop = &doc["properties"]["user"];
        assert!(
            user_prop.get("$ref").is_some(),
            "ref-typed field must render as a $ref: {user_prop}"
        );
    }

    #[test]
    fn input_schema_multimodal_renders_parts_array() {
        // A multimodal input is a single user-supplied `parts` field of type
        // list<variant<… Role::Multimodal>>; it renders as a `parts` array.
        let mut variant = SchemaType::variant(vec![
            VariantCaseType {
                name: "text".to_string(),
                payload: Some(SchemaType::string()),
                metadata: Default::default(),
            },
            VariantCaseType {
                name: "image".to_string(),
                payload: Some(SchemaType::string()),
                metadata: Default::default(),
            },
        ]);
        variant.metadata_mut().role = Some(Role::Multimodal);
        let input = InputSchema::Parameters(vec![NamedField {
            name: "parts".to_string(),
            source: FieldSource::UserSupplied,
            schema: SchemaType::list(variant),
            metadata: Default::default(),
        }]);
        let graph = SchemaGraph::empty();
        let doc =
            input_schema_to_json_schema(&graph, &input, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);
        let parts = &doc["properties"]["parts"];
        assert_eq!(parts["type"], json!("array"));
        assert!(
            parts["items"].get("oneOf").is_some(),
            "multimodal parts items must be a oneOf: {parts}"
        );
        let required = doc["required"].as_array().expect("required array");
        assert!(required.contains(&Value::String("parts".to_string())));
    }

    #[test]
    fn output_schema_unit_is_none_and_single_renders() {
        let graph = SchemaGraph::empty();
        assert!(
            output_schema_to_json_schema(
                &graph,
                &OutputSchema::Unit,
                JsonSchemaConfig::WITHOUT_DRAFT_MARKER
            )
            .is_none()
        );
        let out = OutputSchema::Single(Box::new(SchemaType::u32()));
        let rendered =
            output_schema_to_json_schema(&graph, &out, JsonSchemaConfig::WITHOUT_DRAFT_MARKER)
                .expect("some schema");
        assert_eq!(rendered["type"], json!("integer"));
        assert!(rendered.get("$schema").is_none());
    }

    #[test]
    fn text_with_languages_renders_canonical_shape() {
        use crate::schema::schema_type::TextRestrictions;
        let ty = SchemaType::text(TextRestrictions {
            languages: Some(vec!["en".to_string(), "fr".to_string()]),
            min_length: None,
            max_length: None,
            regex: None,
        });
        let graph = SchemaGraph::anonymous(ty.clone());
        let doc = to_json_schema_with_config(&graph, &ty, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);
        assert_eq!(doc["type"], json!("object"));
        let props = doc["properties"].as_object().expect("properties");
        // Canonical Text shape: `{ text, language? }`.
        assert_eq!(props["text"]["type"], json!("string"));
        assert!(props.contains_key("language"));
        assert_eq!(doc["required"], json!(["text"]));
        assert_eq!(doc["description"], json!("Allowed languages: en, fr"));
    }

    #[test]
    fn multimodal_parts_items_are_canonical_variant_objects() {
        let mut variant = SchemaType::variant(vec![
            VariantCaseType {
                name: "description".to_string(),
                payload: Some(SchemaType::string()),
                metadata: Default::default(),
            },
            VariantCaseType {
                name: "photo".to_string(),
                payload: Some(SchemaType::binary(Default::default())),
                metadata: Default::default(),
            },
        ]);
        variant.metadata_mut().role = Some(Role::Multimodal);
        let input = InputSchema::Parameters(vec![NamedField::user_supplied(
            "parts",
            SchemaType::list(variant),
        )]);
        let graph = SchemaGraph::empty();
        let doc =
            input_schema_to_json_schema(&graph, &input, JsonSchemaConfig::WITHOUT_DRAFT_MARKER);
        let items = &doc["properties"]["parts"]["items"];
        let one_of = items["oneOf"].as_array().expect("oneOf array");
        assert_eq!(one_of.len(), 2);
        // Each branch is the canonical inline variant object `{ <caseName>: <payload> }`,
        // not a `$ref` — and no `$defs` indirection is created.
        assert_eq!(
            one_of[0]["properties"]["description"]["type"],
            json!("string")
        );
        assert_eq!(one_of[0]["required"], json!(["description"]));
        // The binary branch value uses the canonical `{ bytes, mime_type? }` shape.
        assert_eq!(
            one_of[1]["properties"]["photo"]["properties"]["bytes"]["type"],
            json!("string")
        );
        assert_eq!(
            one_of[1]["properties"]["photo"]["required"],
            json!(["bytes"])
        );
        assert!(
            doc.get("$defs").is_none(),
            "multimodal must not synthesise per-branch $defs: {doc}"
        );
    }
}
