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

use super::*;
use golem_common::schema::agent::{AutoInjectedKind, NamedField};
use golem_common::schema::{SchemaTypeDef, TypeId};
use test_r::test;

/// An agent method (or constructor) input schema that mixes user-supplied
/// fields with an auto-injected `principal` field. The value recorded in
/// the oplog is caller-only (the principal travels out of band), so the
/// typed value paired for the public oplog must describe exactly the
/// user-supplied fields — its root record arity must match the value's.
#[test]
fn input_value_to_typed_schema_value_excludes_auto_injected_fields() {
    let input_schema = InputSchema::parameters([
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("label", SchemaType::string()),
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        ),
    ]);
    // Caller-only record as stored in the oplog (two user-supplied values).
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::U32(7), SchemaValue::String("hi".to_string())],
    };

    let typed = input_value_to_typed_schema_value(&SchemaGraph::empty(), &input_schema, value)
        .expect("pairing caller-only value with method schema must succeed");

    let SchemaType::Record { fields, .. } = typed.root_type() else {
        panic!("expected record root, got {:?}", typed.root_type());
    };
    let SchemaValue::Record { fields: values } = typed.value() else {
        panic!("expected record value, got {:?}", typed.value());
    };
    assert_eq!(
        fields.len(),
        values.len(),
        "root record schema arity must match the caller-only value arity"
    );
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["count", "label"]);
}

/// Agent schemas name their records and variants and refer to them through
/// `SchemaType::Ref`. The public typed values must carry the referenced
/// definitions, otherwise consumers cannot resolve field and case names.
fn registry_with_item() -> (SchemaGraph, TypeId) {
    let item_id = TypeId::new("demo.Item");
    let registry = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: item_id.clone(),
                name: Some("Item".to_string()),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "sku".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
            },
            SchemaTypeDef {
                id: TypeId::new("demo.Unrelated"),
                name: Some("Unrelated".to_string()),
                body: SchemaType::u32(),
            },
        ],
        root: SchemaType::tuple(Vec::new()),
    };
    (registry, item_id)
}

#[test]
fn input_value_to_typed_schema_value_carries_reachable_defs() {
    let (registry, item_id) = registry_with_item();
    let input_schema = InputSchema::parameters([NamedField::user_supplied(
        "items",
        SchemaType::list(SchemaType::ref_to(item_id.clone())),
    )]);
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::List {
            elements: vec![SchemaValue::Record {
                fields: vec![SchemaValue::String("SKU-1".to_string())],
            }],
        }],
    };

    let typed = input_value_to_typed_schema_value(&registry, &input_schema, value)
        .expect("pairing caller-only value with method schema must succeed");

    let ids: Vec<&str> = typed.graph().defs.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, vec![item_id.as_str()]);
    let rendered = golem_common::schema::render::value_to_cli_text_with_secret_metadata(
        typed.graph(),
        typed.root_type(),
        typed.value(),
    )
    .expect("named references must resolve in the self-contained graph");
    assert!(rendered.contains("sku"), "{rendered}");
}

#[test]
fn output_value_to_typed_schema_value_carries_reachable_defs() {
    let (registry, item_id) = registry_with_item();
    let output_schema = OutputSchema::Single(Box::new(SchemaType::ref_to(item_id.clone())));
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::String("SKU-1".to_string())],
    };

    let typed = output_value_to_typed_schema_value(&registry, &output_schema, value)
        .expect("pairing output value with method schema must succeed");

    let ids: Vec<&str> = typed.graph().defs.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, vec![item_id.as_str()]);
    assert!(typed.graph().lookup(&item_id).is_some());
}
