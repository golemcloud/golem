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
#![allow(dead_code)]

use golem_common::schema::validation::SchemaError;
use golem_common::schema::{
    FromSchema, IntoSchema, SchemaType, SchemaValue, TextRestrictions, TextValuePayload, TypeId,
    try_into_schema_graph,
};
use test_r::test;

test_r::enable!();

#[derive(IntoSchema)]
#[schema(
    named = "myapp.user",
    doc = "A user record",
    alias = "person",
    example = "{\"name\": \"alice\"}",
    deprecated = "use V2"
)]
struct User {
    #[schema(text(min = 1, max = 100, language = "en"), doc = "Display name")]
    name: String,
}

#[test]
fn type_attributes_populate_metadata_and_typeid() {
    assert_eq!(User::type_id(), TypeId::new("myapp.user"));

    let graph = try_into_schema_graph::<User>().expect("graph should be well-formed");

    let def = graph
        .defs
        .iter()
        .find(|d| d.id == User::type_id())
        .expect("user def is present");

    let def_metadata = def.body.metadata();
    assert_eq!(def_metadata.doc.as_deref(), Some("A user record"));
    assert_eq!(def_metadata.aliases, vec!["person".to_string()]);
    assert_eq!(
        def_metadata.examples,
        vec!["{\"name\": \"alice\"}".to_string()]
    );
    assert_eq!(def_metadata.deprecated.as_deref(), Some("use V2"));
    assert_eq!(def.name.as_deref(), Some("myapp.user"));

    match &def.body {
        SchemaType::Record { fields, .. } => {
            assert_eq!(fields[0].name, "name");
            assert_eq!(fields[0].metadata.doc.as_deref(), Some("Display name"));
            match &fields[0].body {
                SchemaType::Text { restrictions, .. } => {
                    assert_eq!(
                        restrictions,
                        &TextRestrictions {
                            languages: Some(vec!["en".to_string()]),
                            min_length: Some(1),
                            max_length: Some(100),
                            regex: None,
                        }
                    );
                }
                other => panic!("expected text restrictions, got {other:?}"),
            }
        }
        other => panic!("expected record body, got {other:?}"),
    }
}

#[test]
fn user_to_value_emits_text_payload() {
    let u = User {
        name: "alice".to_string(),
    };
    let v = u.to_value();
    let SchemaValue::Record { fields } = v else {
        panic!("expected record");
    };
    assert_eq!(
        fields[0],
        SchemaValue::Text(TextValuePayload {
            text: "alice".to_string(),
            language: Some("en".to_string()),
        })
    );
}

#[derive(IntoSchema)]
struct WithSecretField {
    #[schema(secret)]
    credential: String,
}

#[test]
fn secret_field_attribute_defaults_inner_to_string() {
    let graph = try_into_schema_graph::<WithSecretField>().expect("graph should be well-formed");
    let def = graph
        .defs
        .iter()
        .find(|d| d.id == WithSecretField::type_id())
        .expect("with-secret def is present");

    match &def.body {
        SchemaType::Record { fields, .. } => match &fields[0].body {
            SchemaType::Secret { spec, .. } => {
                assert_eq!(spec.inner.as_ref(), &SchemaType::string());
                assert_eq!(spec.category, None);
            }
            other => panic!("expected secret field, got {other:?}"),
        },
        other => panic!("expected record body, got {other:?}"),
    }
}

#[derive(IntoSchema)]
struct WithRename {
    #[schema(rename = "real-name")]
    snake_name: String,
}

#[test]
fn field_rename_attribute_overrides_default_name() {
    let graph = try_into_schema_graph::<WithRename>().expect("graph should be well-formed");
    let def = &graph.defs[0];
    match &def.body {
        SchemaType::Record { fields, .. } => assert_eq!(fields[0].name, "real-name"),
        other => panic!("expected record, got {other:?}"),
    }
}

#[derive(IntoSchema)]
struct Source {
    #[schema(source = "auto_injected", kind = "principal")]
    principal: String,
}

#[test]
fn source_and_kind_attributes_are_accepted_and_ignored_today() {
    // Today these attributes are accepted by the derive but do not surface in
    // the emitted SchemaType — that wiring lands together with the future
    // `FieldSource` type.
    let graph = try_into_schema_graph::<Source>().expect("graph should be well-formed");
    match &graph.defs[0].body {
        SchemaType::Record { fields, .. } => assert_eq!(fields[0].name, "principal"),
        other => panic!("expected record, got {other:?}"),
    }
}

#[derive(IntoSchema)]
#[schema(role = "multimodal")]
struct Multimodal {
    items: Vec<String>,
}

#[test]
fn role_attribute_populates_metadata_role() {
    let graph = try_into_schema_graph::<Multimodal>().expect("graph should be well-formed");
    let def = &graph.defs[0];
    assert_eq!(
        def.body.metadata().role.as_ref().map(|r| format!("{r:?}")),
        Some("Multimodal".to_string())
    );
}

// ----------------------------------------------------------------------
// New attributes: rename_all
// ----------------------------------------------------------------------

#[derive(IntoSchema)]
#[schema(rename_all = "snake_case")]
struct SnakeNames {
    foo_bar: u32,
    baz_qux: String,
}

#[test]
fn rename_all_snake_case() {
    let graph = try_into_schema_graph::<SnakeNames>().expect("graph should be well-formed");
    match &graph.defs[0].body {
        SchemaType::Record { fields, .. } => {
            assert_eq!(fields[0].name, "foo_bar");
            assert_eq!(fields[1].name, "baz_qux");
        }
        other => panic!("expected record, got {other:?}"),
    }
}

// ----------------------------------------------------------------------
// New attributes: skip + default = "..."
// ----------------------------------------------------------------------

fn default_x() -> u32 {
    42
}

#[derive(IntoSchema, FromSchema, Debug, PartialEq)]
struct WithSkipped {
    keep: u32,
    #[schema(skip)]
    drop_me: u32,
    #[schema(default = "default_x")]
    use_default: u32,
}

#[test]
fn skip_attribute_omits_field_from_schema_and_value() {
    let graph = try_into_schema_graph::<WithSkipped>().expect("graph should be well-formed");
    match &graph.defs[0].body {
        SchemaType::Record { fields, .. } => {
            // Skipped and defaulted fields don't appear in the schema.
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].name, "keep");
        }
        other => panic!("expected record, got {other:?}"),
    }

    let value = SchemaValue::Record {
        fields: vec![SchemaValue::U32(7)],
    };
    let decoded = WithSkipped::from_value(&value).expect("decode succeeds");
    assert_eq!(
        decoded,
        WithSkipped {
            keep: 7,
            drop_me: 0,
            use_default: 42,
        }
    );
}

// ----------------------------------------------------------------------
// Rich-scalar field round-trip: text round-trip
// ----------------------------------------------------------------------

#[derive(IntoSchema, FromSchema, Debug, PartialEq)]
struct WithText {
    #[schema(text(language = "en"))]
    message: String,
}

#[test]
fn text_field_round_trips_through_value() {
    let original = WithText {
        message: "hello".to_string(),
    };
    let v = original.to_value();
    let decoded = WithText::from_value(&v).expect("decode succeeds");
    assert_eq!(decoded, original);

    // Confirm the value side really uses the Text payload.
    let SchemaValue::Record { fields } = &v else {
        panic!("expected record");
    };
    assert_eq!(
        fields[0],
        SchemaValue::Text(TextValuePayload {
            text: "hello".to_string(),
            language: Some("en".to_string()),
        })
    );
}

// ----------------------------------------------------------------------
// §4.19 — nullable nesting rejected at schema construction
// ----------------------------------------------------------------------

#[derive(IntoSchema)]
struct WithNestedOption {
    inner: Option<Option<i32>>,
}

#[test]
fn try_into_schema_graph_rejects_nested_option() {
    let err = try_into_schema_graph::<WithNestedOption>()
        .expect_err("nested option should fail well-formedness validation");
    match err {
        SchemaError::NullableNesting { .. } => {}
        other => panic!("expected NullableNesting, got {other:?}"),
    }
}
