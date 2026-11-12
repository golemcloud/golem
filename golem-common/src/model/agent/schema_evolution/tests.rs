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
use crate::schema::agent::{
    AgentConstructorSchema, AgentTypeSchema, AutoInjectedKind, FieldSource, InputSchema, NamedField,
};
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

/// Compares the constructors of the agent types present in both lists, as the
/// registry does for each deployed type that has a stored baseline.
fn validate_schema_evolution(
    old: &[AgentTypeSchema],
    new: &[AgentTypeSchema],
) -> Vec<SchemaEvolutionError> {
    old.iter()
        .filter_map(|o| {
            new.iter()
                .find(|n| n.type_name == o.type_name)
                .map(|n| (o, n))
        })
        .flat_map(|(o, n)| {
            validate_constructor_evolution(
                &o.type_name,
                &constructor_key_schema(o),
                &constructor_key_schema(n),
            )
        })
        .collect()
}

#[test]
fn no_changes_no_errors() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn agent_type_added_no_error() {
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
fn record_append_non_option_field_changes_keys() {
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
fn tuple_append_option_changes_keys() {
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
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("items added"));
}

#[test]
fn root_append_option_parameter_compatible() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![make_agent_type(
        "A",
        vec![
            ("x", SchemaType::u32()),
            ("y", SchemaType::option(SchemaType::string())),
        ],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn root_append_non_option_parameter_changes_keys() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let new = vec![make_agent_type(
        "A",
        vec![("x", SchemaType::u32()), ("y", SchemaType::string())],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].path, "constructor.fields[1]");
    assert!(w[0].description.contains("non-Option"));
}

#[test]
fn auto_injected_parameters_are_not_part_of_the_key() {
    let old = vec![make_agent_type("A", vec![("x", SchemaType::u32())])];
    let mut new_type = make_agent_type(
        "A",
        vec![
            ("principal", SchemaType::string()),
            ("x", SchemaType::u32()),
        ],
    );
    let InputSchema::Parameters(fields) = &mut new_type.constructor.input_schema;
    fields[0].source = FieldSource::AutoInjected(AutoInjectedKind::Principal);
    assert!(validate_schema_evolution(&old, &[new_type]).is_empty());
}

fn union_of(bodies: Vec<SchemaType>) -> SchemaType {
    use crate::schema::schema_type::{DiscriminatorRule, UnionBranch, UnionSpec};
    SchemaType::union(UnionSpec {
        branches: bodies
            .into_iter()
            .enumerate()
            .map(|(i, body)| UnionBranch {
                tag: format!("b{i}"),
                body,
                discriminator: DiscriminatorRule::FieldAbsent {
                    field_name: format!("b{i}"),
                },
                metadata: Default::default(),
            })
            .collect(),
    })
}

#[test]
fn union_branch_removed_changes_keys() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", union_of(vec![SchemaType::u32(), SchemaType::string()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", union_of(vec![SchemaType::u32()]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert!(w[0].description.contains("Union branches removed"));
}

#[test]
fn union_branch_body_changed_changes_keys() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", union_of(vec![SchemaType::u32()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", union_of(vec![SchemaType::string()]))],
    )];
    let w = validate_schema_evolution(&old, &new);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].path, "constructor.fields[0].branches[0]");
}

#[test]
fn union_branch_appended_compatible() {
    let old = vec![make_agent_type(
        "A",
        vec![("x", union_of(vec![SchemaType::u32()]))],
    )];
    let new = vec![make_agent_type(
        "A",
        vec![("x", union_of(vec![SchemaType::u32(), SchemaType::string()]))],
    )];
    assert!(validate_schema_evolution(&old, &new).is_empty());
}

#[test]
fn tuple_append_non_option_changes_keys() {
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
    assert!(w[0].description.contains("items added"));
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

/// Every integer and float type pair, with whether the checker accepts the change.
/// The test also checks the decision against the key text itself: for an accepted
/// change, the smallest and largest value of the old type keep their text under the
/// new type; for a rejected change, at least one of them does not.
#[test]
fn numeric_type_changes_follow_the_key_text() {
    use crate::model::agent::structural_format::{format_structural_typed, parse_structural_typed};
    use crate::schema::graph::TypedSchemaValue;

    let integers = [
        (SchemaType::s8(), i8::MIN as i128, i8::MAX as i128),
        (SchemaType::s16(), i16::MIN as i128, i16::MAX as i128),
        (SchemaType::s32(), i32::MIN as i128, i32::MAX as i128),
        (SchemaType::s64(), i64::MIN as i128, i64::MAX as i128),
        (SchemaType::u8(), 0, u8::MAX as i128),
        (SchemaType::u16(), 0, u16::MAX as i128),
        (SchemaType::u32(), 0, u32::MAX as i128),
        (SchemaType::u64(), 0, u64::MAX as i128),
    ];
    let accepted = |old: &SchemaType, new: &SchemaType| {
        validate_schema_evolution(
            &[make_agent_type("A", vec![("x", old.clone())])],
            &[make_agent_type("A", vec![("x", new.clone())])],
        )
        .is_empty()
    };
    let keeps_text = |text: &str, new: &SchemaType| {
        let graph = SchemaGraph::anonymous(rec(vec![("x", new.clone())]));
        parse_structural_typed(text, &graph, &graph.root)
            .and_then(|value| format_structural_typed(&TypedSchemaValue::new(graph, value)))
            .is_ok_and(|formatted| formatted == text)
    };

    let mut accepted_pairs = Vec::new();
    for (old, min, max) in &integers {
        for (new, _, _) in &integers {
            let texts_kept = [min, max]
                .iter()
                .all(|bound| keeps_text(&bound.to_string(), new));
            assert_eq!(
                accepted(old, new),
                texts_kept,
                "{old:?} -> {new:?}: the checker and the key text disagree"
            );
            if texts_kept {
                accepted_pairs.push((discriminant_name(old), discriminant_name(new)));
            }
        }
    }
    assert_eq!(
        accepted_pairs,
        vec![
            ("S8", "S8"),
            ("S8", "S16"),
            ("S8", "S32"),
            ("S8", "S64"),
            ("S16", "S16"),
            ("S16", "S32"),
            ("S16", "S64"),
            ("S32", "S32"),
            ("S32", "S64"),
            ("S64", "S64"),
            ("U8", "S16"),
            ("U8", "S32"),
            ("U8", "S64"),
            ("U8", "U8"),
            ("U8", "U16"),
            ("U8", "U32"),
            ("U8", "U64"),
            ("U16", "S32"),
            ("U16", "S64"),
            ("U16", "U16"),
            ("U16", "U32"),
            ("U16", "U64"),
            ("U32", "S64"),
            ("U32", "U32"),
            ("U32", "U64"),
            ("U64", "U64"),
        ]
    );

    assert!(!keeps_text("0.1", &SchemaType::u8()));
    for (old, new) in [
        (SchemaType::f32(), SchemaType::f64()),
        (SchemaType::f64(), SchemaType::f32()),
        (SchemaType::u8(), SchemaType::f64()),
        (SchemaType::f32(), SchemaType::s64()),
    ] {
        assert!(!accepted(&old, &new), "{old:?} -> {new:?} must be rejected");
    }
}
