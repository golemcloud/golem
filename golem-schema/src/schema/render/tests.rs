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

use super::{RenderError, from_json_value, to_json_schema, to_json_value};
use crate::schema::{
    MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue,
    TextRestrictions, TextValuePayload, TypeId, VariantCaseType, VariantValuePayload,
};
use serde_json::{Value, json};
use test_r::test;

#[test]
fn canonical_record_round_trips_through_json() {
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u64(),
            metadata: MetadataEnvelope::default(),
        },
        NamedFieldType {
            name: "name".to_string(),
            body: SchemaType::text(TextRestrictions::default()),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U64(u64::MAX),
            SchemaValue::Text(TextValuePayload {
                text: "Ada".to_string(),
                language: Some("en".to_string()),
            }),
        ],
    };

    let rendered = to_json_value(&graph, &ty, &value).expect("render record");
    assert_eq!(
        rendered,
        json!({
            "id": u64::MAX,
            "name": { "text": "Ada", "language": "en" }
        })
    );
    assert_eq!(
        from_json_value(&graph, &ty, &rendered).expect("decode record"),
        value
    );
}

#[test]
fn refs_variants_and_options_share_one_graph() {
    let payload_id = TypeId::new("example.payload");
    let payload = SchemaType::record(vec![NamedFieldType {
        name: "value".to_string(),
        body: SchemaType::option(SchemaType::string()),
        metadata: MetadataEnvelope::default(),
    }]);
    let root = SchemaType::variant(vec![
        VariantCaseType {
            name: "empty".to_string(),
            payload: None,
            metadata: MetadataEnvelope::default(),
        },
        VariantCaseType {
            name: "payload".to_string(),
            payload: Some(SchemaType::ref_to(payload_id.clone())),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: payload_id,
            name: Some("Payload".to_string()),
            body: payload,
        }],
        root: root.clone(),
    };
    let value = SchemaValue::Variant(VariantValuePayload {
        case: 1,
        payload: Some(Box::new(SchemaValue::Record {
            fields: vec![SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("x".to_string()))),
            }],
        })),
    });

    let rendered = to_json_value(&graph, &root, &value).expect("render variant");
    assert_eq!(rendered, json!({ "payload": { "value": "x" } }));
    assert_eq!(
        from_json_value(&graph, &root, &rendered).expect("decode variant"),
        value
    );

    let schema = to_json_schema(&graph, &root);
    assert_eq!(
        schema["$schema"],
        json!("https://json-schema.org/draft/2020-12/schema")
    );
    assert!(schema["$defs"].get("example.payload").is_some());
    let required = schema["$defs"]["example.payload"]["required"]
        .as_array()
        .expect("required list");
    assert!(!required.contains(&Value::String("value".to_string())));
}

#[test]
fn malformed_json_and_schema_values_are_typed_errors() {
    let ty = SchemaType::record(vec![NamedFieldType {
        name: "id".to_string(),
        body: SchemaType::u32(),
        metadata: MetadataEnvelope::default(),
    }]);
    let graph = SchemaGraph::anonymous(ty.clone());

    let unexpected = from_json_value(&graph, &ty, &json!({ "id": 1, "extra": true }))
        .expect_err("extra field must fail");
    assert!(matches!(unexpected, RenderError::UnexpectedField { .. }));

    let mismatch = to_json_value(&graph, &ty, &SchemaValue::Bool(true))
        .expect_err("wrong value shape must fail");
    assert!(matches!(mismatch, RenderError::ValueMismatch { .. }));
}
