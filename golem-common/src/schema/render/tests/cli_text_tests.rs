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

use crate::schema::canonical;
use crate::schema::graph::SchemaGraph;
use crate::schema::render::cli_text::{
    type_to_cli_text, value_to_cli_text, value_to_cli_text_unredacted,
};
use crate::schema::schema_type::{NamedFieldType, SchemaType, SecretSpec, TextRestrictions};
use crate::schema::schema_value::{SchemaValue, SecretValuePayload, TextValuePayload};
use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use test_r::test;

#[test]
fn type_record_renders_concise_form() {
    let ty = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "id".to_string(),
                body: SchemaType::U32,
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "name".to_string(),
                body: SchemaType::Text(TextRestrictions {
                    min_length: Some(1),
                    max_length: Some(100),
                    ..Default::default()
                }),
                metadata: Default::default(),
            },
        ],
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let text = type_to_cli_text(&graph, &ty);
    assert_eq!(text, "record { id: u32, name: text(min=1, max=100) }");
}

#[test]
fn type_list_renders_with_angles() {
    let ty = SchemaType::List {
        element: Box::new(SchemaType::Text(TextRestrictions::default())),
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    assert_eq!(type_to_cli_text(&graph, &ty), "list<text>");
}

#[test]
fn type_secret_renders_as_secret() {
    let ty = SchemaType::Secret(crate::schema::schema_type::SecretSpec::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    assert_eq!(type_to_cli_text(&graph, &ty), "secret");
}

#[test]
fn value_record_renders_concise_form() {
    let ty = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "id".to_string(),
                body: SchemaType::U32,
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "name".to_string(),
                body: SchemaType::Text(TextRestrictions::default()),
                metadata: Default::default(),
            },
        ],
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(7),
            SchemaValue::Text(TextValuePayload {
                text: "Ada".to_string(),
                language: None,
            }),
        ],
    };
    let text = value_to_cli_text(&graph, &ty, &value).expect("value_to_cli_text");
    assert_eq!(text, "{ id: 7, name: Ada }");
}

#[test]
fn value_list_renders_with_brackets() {
    let ty = SchemaType::List {
        element: Box::new(SchemaType::U32),
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::List {
        elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)],
    };
    let text = value_to_cli_text(&graph, &ty, &value).expect("value_to_cli_text");
    assert_eq!(text, "[1, 2]");
}

#[test]
fn secret_value_is_redacted_by_default() {
    let ty = SchemaType::Secret(SecretSpec::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Secret(SecretValuePayload {
        secret_ref: "shhh".to_string(),
    });
    let text = value_to_cli_text(&graph, &ty, &value).expect("value_to_cli_text");
    assert_eq!(text, "<redacted>");
    let unredacted =
        value_to_cli_text_unredacted(&graph, &ty, &value).expect("value_to_cli_text_unredacted");
    assert!(unredacted.starts_with("secret:"));
}

#[test]
fn text_value_emits_raw_canonical_form() {
    let ty = SchemaType::Text(TextRestrictions::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Text(TextValuePayload {
        text: "hello".to_string(),
        language: None,
    });
    let text = value_to_cli_text(&graph, &ty, &value).expect("value_to_cli_text");
    // Raw canonical form, not Rust debug quoting.
    assert_eq!(text, "hello");
}

#[test]
fn type_to_cli_text_includes_text_regex_and_languages() {
    let ty = SchemaType::Text(TextRestrictions {
        languages: Some(vec!["en".to_string(), "fr".to_string()]),
        min_length: None,
        max_length: None,
        regex: Some("^[a-z]+$".to_string()),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let text = type_to_cli_text(&graph, &ty);
    assert!(text.contains("regex="));
    assert!(text.contains("languages="));
}

#[test]
fn datetime_value_uses_canonical_form() {
    let ty = SchemaType::Datetime;
    let graph = SchemaGraph::anonymous(ty.clone());
    let dt = Utc.timestamp_opt(0, 0).single().unwrap();
    let value = SchemaValue::Datetime { value: dt };
    let text = value_to_cli_text(&graph, &ty, &value).expect("value_to_cli_text");
    assert_eq!(text, canonical::datetime::to_text(&dt).unwrap());
}

// Primitive + rich scalar round-trip property tests.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn primitive_text_round_trip(s in "[ -~]{0,16}") {
        let payload = TextValuePayload { text: s, language: None };
        let text = canonical::text::to_text(&payload);
        let back = canonical::text::from_text(&text).expect("from_text");
        prop_assert_eq!(payload, back);
    }

    #[test]
    fn primitive_path_round_trip(s in "[a-zA-Z][a-zA-Z0-9/._-]{0,16}") {
        let rendered = canonical::path::to_text(&s).expect("to_text");
        let back = canonical::path::from_text(&rendered).expect("from_text");
        prop_assert_eq!(s, back);
    }

    #[test]
    fn primitive_url_round_trip(s in "[a-z]+://[a-z][a-z0-9./]{0,16}") {
        let rendered = canonical::url::to_text(&s).expect("to_text");
        let back = canonical::url::from_text(&rendered).expect("from_text");
        prop_assert_eq!(s, back);
    }
}
