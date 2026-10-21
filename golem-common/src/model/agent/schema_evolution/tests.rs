// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use crate::base_model::agent::{AgentMode, AgentTypeName, Snapshotting};
use crate::model::Empty;
use crate::schema::agent::{AgentConstructorSchema, NamedField};
use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::{ResultSpec, SchemaType, VariantCaseType};
use test_r::test;

fn field(name: &str, schema: SchemaType) -> NamedField {
    NamedField {
        name: name.to_string(),
        source: Default::default(),
        schema,
        metadata: Default::default(),
    }
}

fn rec(fields: Vec<(&str, SchemaType)>) -> SchemaType {
    SchemaType::record(
        fields
            .into_iter()
            .map(|(n, t)| crate::schema::schema_type::NamedFieldType {
                name: n.to_string(),
                body: t,
                metadata: Default::default(),
            })
            .collect(),
    )
}

fn unit_case(name: &str) -> VariantCaseType {
    VariantCaseType {
        name: name.to_string(),
        payload: None,
        metadata: Default::default(),
    }
}

fn case(name: &str, payload: SchemaType) -> VariantCaseType {
    VariantCaseType {
        name: name.to_string(),
        payload: Some(payload),
        metadata: Default::default(),
    }
}

fn result_ok(ok: SchemaType) -> SchemaType {
    SchemaType::result(ResultSpec {
        ok: Some(Box::new(ok)),
        err: None,
    })
}

fn result(ok: SchemaType, err: SchemaType) -> SchemaType {
    SchemaType::result(ResultSpec {
        ok: Some(Box::new(ok)),
        err: Some(Box::new(err)),
    })
}

fn make_agent_type(name: &str, fields: Vec<(&str, SchemaType)>) -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: AgentTypeName(name.to_string()),
        kind: crate::schema::agent::AgentTypeKind::Regular,
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(
                fields.into_iter().map(|(n, t)| field(n, t)).collect(),
            ),
        },
        methods: Vec::new(),
        dependencies: Vec::new(),
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: Vec::new(),
    }
}

#[test]
fn no_changes_no_warnings() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn agent_type_removed() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("removed"));
}

#[test]
fn agent_type_added_no_warning() {
    let old = vec![];
    let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn element_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::u32()), ("y", SchemaType::string())],
    )];
    let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("removed"));
}

#[test]
fn element_type_changed() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![make_agent_type("A", vec![("x", SchemaType::string())])];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn record_field_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![(
            "x",
            rec(vec![("a", SchemaType::u32()), ("b", SchemaType::string())]),
        )],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", rec(vec![("a", SchemaType::u32())]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("fields removed"));
}

#[test]
fn record_field_type_changed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", rec(vec![("a", SchemaType::u32())]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", rec(vec![("a", SchemaType::string())]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn record_append_option_field_compatible() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", rec(vec![("a", SchemaType::u32())]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            rec(vec![
                ("a", SchemaType::u32()),
                ("b", SchemaType::option(SchemaType::string())),
            ]),
        )],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn record_append_non_option_field_warns() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", rec(vec![("a", SchemaType::u32())]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            rec(vec![("a", SchemaType::u32()), ("b", SchemaType::string())]),
        )],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("non-Option"));
}

#[test]
fn variant_case_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::variant(vec![unit_case("A"), unit_case("B")]),
        )],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![unit_case("A")]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("cases removed"));
}

#[test]
fn variant_payload_added() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![unit_case("A")]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![case("A", SchemaType::u32())]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("gained a payload"));
}

#[test]
fn variant_payload_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![case("A", SchemaType::u32())]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![unit_case("A")]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("lost its payload"));
}

#[test]
fn variant_payload_type_changed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![case("A", SchemaType::u32())]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::variant(vec![case("A", SchemaType::string())]),
        )],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn variant_case_appended_compatible() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::variant(vec![unit_case("A")]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::variant(vec![unit_case("A"), unit_case("B")]),
        )],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn enum_cases_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::r#enum(vec!["A".into(), "B".into(), "C".into()]),
        )],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::r#enum(vec!["A".into(), "B".into()]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Enum cases removed"));
}

#[test]
fn enum_cases_appended_compatible() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::r#enum(vec!["A".into(), "B".into()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::r#enum(vec!["A".into(), "B".into(), "C".into()]),
        )],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn flags_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
        )],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::flags(vec!["a".into(), "b".into()]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Flags removed"));
}

#[test]
fn flags_appended_compatible() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::flags(vec!["a".into(), "b".into()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
        )],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn tuple_item_removed() {
    let old = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::tuple(vec![SchemaType::u32(), SchemaType::string()]),
        )],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::tuple(vec![SchemaType::u32()]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("items removed"));
}

#[test]
fn tuple_append_option_compatible() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::tuple(vec![SchemaType::u32()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::tuple(vec![
                SchemaType::u32(),
                SchemaType::option(SchemaType::string()),
            ]),
        )],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn tuple_append_non_option_warns() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::tuple(vec![SchemaType::u32()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![(
            "x",
            SchemaType::tuple(vec![SchemaType::u32(), SchemaType::string()]),
        )],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("non-Option"));
}

#[test]
fn list_inner_type_changed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::list(SchemaType::u32()))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::list(SchemaType::string()))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn option_inner_type_changed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::option(SchemaType::u32()))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::option(SchemaType::string()))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn result_ok_type_changed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", result_ok(SchemaType::u32()))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", result_ok(SchemaType::string()))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn result_err_presence_changed() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", result_ok(SchemaType::u32()))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", result(SchemaType::u32(), SchemaType::string()))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("err payload presence"));
}

#[test]
fn primitive_type_changed() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![make_agent_type("A", vec![("x", SchemaType::string())])];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Type changed"));
}

#[test]
fn multiple_warnings() {
    let old = vec![make_agent_type(
        "A",
        vec![
            ("x", SchemaType::u32()),
            (
                "y",
                rec(vec![("a", SchemaType::string()), ("b", SchemaType::bool())]),
            ),
        ],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![
            ("x", SchemaType::string()),
            ("y", rec(vec![("a", SchemaType::u8())])),
        ],
    )];
    let w = validate_schema_evolution(&old, &new);
    // x: type changed (u32 -> str)
    // y: record fields removed (2 -> 1)
    assert_eq!(w.len(), 2);
}
