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

use crate::base_model::Empty;
use crate::base_model::agent::{AgentMode, AgentTypeName, Snapshotting};
use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentTypeSchema, AutoInjectedKind, FieldSource,
    InputSchema, NamedField, OutputSchema, ParsedAgentId,
    json_input_schema_value_to_typed_schema_value, typed_schema_value_with_projected_defs,
};
use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{NamedFieldType, SchemaType, VariantCaseType};
use crate::schema::schema_value::{SchemaValue, VariantValuePayload};
use proptest::prelude::*;
use serde_json::json;
use test_r::test;
use uuid::Uuid;

// --- Builders / convenience ---

#[test]
fn input_schema_parameters_builder_collects_fields() {
    let s = InputSchema::parameters([
        NamedField::user_supplied("name", SchemaType::string()),
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        ),
    ]);
    assert_eq!(s.fields().len(), 2);
    assert_eq!(s.fields()[0].name, "name");
    assert!(matches!(s.fields()[0].source, FieldSource::UserSupplied));
    assert!(matches!(
        s.fields()[1].source,
        FieldSource::AutoInjected(AutoInjectedKind::Principal)
    ));
}

#[test]
fn output_schema_schema_returns_none_for_unit() {
    assert!(OutputSchema::Unit.schema().is_none());
    let s = OutputSchema::Single(Box::new(SchemaType::s32()));
    assert!(matches!(s.schema(), Some(SchemaType::S32 { .. })));
}

// --- Serde shape pins (so wire format changes are caught) ---

#[test]
fn input_schema_parameters_serde_shape() {
    let s = InputSchema::Parameters(vec![NamedField::user_supplied(
        "name",
        SchemaType::string(),
    )]);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["tag"], "parameters");
    assert!(v["value"].is_array());
    assert_eq!(v["value"][0]["name"], "name");
    assert_eq!(v["value"][0]["source"]["tag"], "user-supplied");
}

#[test]
fn output_schema_unit_serde_shape() {
    let v = serde_json::to_value(OutputSchema::Unit).unwrap();
    assert_eq!(v, json!({"tag": "unit"}));
}

#[test]
fn output_schema_single_serde_shape() {
    let v = serde_json::to_value(OutputSchema::Single(Box::new(SchemaType::s32()))).unwrap();
    assert_eq!(v["tag"], "single");
    assert!(v["value"].is_object());
}

#[test]
fn field_source_user_supplied_serde_shape() {
    let v = serde_json::to_value(FieldSource::UserSupplied).unwrap();
    assert_eq!(v, json!({"tag": "user-supplied"}));
}

#[test]
fn field_source_auto_injected_serde_shape() {
    let v = serde_json::to_value(FieldSource::AutoInjected(AutoInjectedKind::Principal)).unwrap();
    assert_eq!(v, json!({"tag": "auto-injected", "value": "principal"}));
}

#[test]
fn auto_injected_kind_serde_is_kebab_case_string() {
    let v = serde_json::to_value(AutoInjectedKind::Principal).unwrap();
    assert_eq!(v, json!("principal"));
}

// --- Round-trips for fixed shapes ---

#[test]
fn input_schema_round_trip_mixed_sources() {
    let s = InputSchema::Parameters(vec![
        NamedField::user_supplied("name", SchemaType::string()),
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        ),
    ]);
    let json = serde_json::to_string(&s).unwrap();
    let back: InputSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
}

#[test]
fn output_schema_round_trip_unit() {
    let s = OutputSchema::Unit;
    let json = serde_json::to_string(&s).unwrap();
    let back: OutputSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
}

#[test]
fn named_field_with_metadata_round_trips() {
    let field = NamedField {
        name: "amount".into(),
        source: FieldSource::UserSupplied,
        schema: SchemaType::u64(),
        metadata: MetadataEnvelope {
            doc: Some("How much.".into()),
            aliases: vec!["qty".into()],
            examples: vec!["42".into()],
            deprecated: None,
            role: None,
        },
    };
    let json = serde_json::to_string(&field).unwrap();
    let back: NamedField = serde_json::from_str(&json).unwrap();
    assert_eq!(back, field);
}

// --- Property: ParsedAgentId fields the type itself owns round-trip ---

// JSON serde of arbitrary `SchemaValue` trees is not a guaranteed bijection
// (the schema layer does not adopt JSON-as-wire; see §4.16). These properties
// only exercise the fields `ParsedAgentId` itself owns, with a fixed inner
// `TypedSchemaValue`.

fn sample_parameters() -> TypedSchemaValue {
    TypedSchemaValue::new(
        crate::schema::graph::SchemaGraph::anonymous(SchemaType::string()),
        SchemaValue::String("alice".into()),
    )
}

proptest! {
    #[test]
    fn parsed_agent_id_phantom_id_round_trips(
        phantom_present in any::<bool>(),
    ) {
        let phantom_id = phantom_present.then(Uuid::new_v4);
        let id = ParsedAgentId::new(
            AgentTypeName("weather-agent".into()),
            sample_parameters(),
            phantom_id,
        );
        let json = serde_json::to_string(&id).unwrap();
        let back: ParsedAgentId = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back, id);
    }

    #[test]
    fn parsed_agent_id_agent_type_round_trips(
        type_name in "[a-z][a-z0-9-]{0,32}",
    ) {
        let id = ParsedAgentId::new(
            AgentTypeName(type_name),
            sample_parameters(),
            None,
        );
        let json = serde_json::to_string(&id).unwrap();
        let back: ParsedAgentId = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back, id);
    }
}

// --- Reject malformed wire forms ---

#[test]
fn parsed_agent_id_rejects_missing_parameters() {
    let raw = json!({
        "agent_type": "weather-agent",
        "phantom_id": null
    });
    let res: Result<ParsedAgentId, _> = serde_json::from_value(raw);
    assert!(res.is_err());
}

#[test]
fn parsed_agent_id_accepts_absent_phantom_id() {
    let parameters = TypedSchemaValue::new(
        crate::schema::graph::SchemaGraph::anonymous(SchemaType::string()),
        SchemaValue::String("alice".into()),
    );
    let json = serde_json::to_value(ParsedAgentId::new(
        AgentTypeName("weather-agent".into()),
        parameters.clone(),
        None,
    ))
    .unwrap();
    let mut obj = json.as_object().unwrap().clone();
    obj.remove("phantom_id");
    let back: ParsedAgentId = serde_json::from_value(serde_json::Value::Object(obj)).unwrap();
    assert!(back.phantom_id.is_none());
}

// --- SchemaGraph::empty() and agent-layer carrier ---

#[test]
fn schema_graph_empty_has_no_defs_and_placeholder_root() {
    let g = SchemaGraph::empty();
    assert!(g.defs.is_empty());
    match g.root {
        SchemaType::Record { ref fields, .. } => assert!(fields.is_empty()),
        ref other => panic!("expected empty Record sentinel root, got: {other:?}"),
    }
}

fn sample_agent_type() -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: AgentTypeName("weather-agent".into()),
        description: "An agent".into(),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    }
}

#[test]
fn agent_type_schema_default_schema_round_trips_through_omission() {
    // Older payloads omit the `schema` field; new code must decode them
    // as `SchemaGraph::empty()`.
    let mut value = serde_json::to_value(sample_agent_type()).unwrap();
    let obj = value.as_object_mut().unwrap();
    obj.remove("schema");
    let back: AgentTypeSchema = serde_json::from_value(value).unwrap();
    assert_eq!(back.schema, SchemaGraph::empty());
}

#[test]
fn agent_dependency_schema_default_schema_round_trips_through_omission() {
    let dep = AgentDependencySchema {
        type_name: "dep".into(),
        description: None,
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
    };
    let mut value = serde_json::to_value(&dep).unwrap();
    let obj = value.as_object_mut().unwrap();
    obj.remove("schema");
    let back: AgentDependencySchema = serde_json::from_value(value).unwrap();
    assert_eq!(back.schema, SchemaGraph::empty());
}

#[test]
fn agent_type_schema_round_trip_preserves_schema_graph() {
    // A populated graph (one named def + non-empty root) must round-trip
    // verbatim. Carriers that use the graph as a registry simply ignore
    // the root, but serde must not drop it.
    let agent = AgentTypeSchema {
        schema: SchemaGraph::anonymous(SchemaType::s32()),
        ..sample_agent_type()
    };
    let json = serde_json::to_string(&agent).unwrap();
    let back: AgentTypeSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(back, agent);
}

// --- json_input_schema_value_to_typed_schema_value: def projection (Option 7) ---

fn proj_def(id: &str, body: SchemaType) -> SchemaTypeDef {
    SchemaTypeDef {
        id: TypeId::new(id),
        name: None,
        body,
    }
}

fn proj_field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

/// A multi-root agent registry: only `defs` matter; `root` is a placeholder.
fn registry(defs: Vec<SchemaTypeDef>) -> SchemaGraph {
    SchemaGraph {
        defs,
        root: SchemaType::record(vec![]),
    }
}

fn proj_ids(typed: &TypedSchemaValue) -> Vec<String> {
    typed
        .graph()
        .defs
        .iter()
        .map(|d| d.id.as_str().to_string())
        .collect()
}

#[test]
fn json_input_projects_away_unreachable_defs() {
    // Primitive-only input references no defs, so none of the registry's defs
    // should survive in the self-contained result.
    let graph = registry(vec![
        proj_def(
            "Unused1",
            SchemaType::record(vec![proj_field("a", SchemaType::u32())]),
        ),
        proj_def("Unused2", SchemaType::string()),
    ]);
    let input_schema =
        InputSchema::parameters([NamedField::user_supplied("seed", SchemaType::u64())]);
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::U64(7)],
    })
    .unwrap();

    let typed = json_input_schema_value_to_typed_schema_value(json, &graph, &input_schema).unwrap();

    assert!(typed.graph().defs.is_empty());
    assert_eq!(
        typed.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U64(7)],
        }
    );
}

#[test]
fn json_input_keeps_transitively_reachable_defs_in_order() {
    // A = record { b: Ref(B) }, B = record { s: string }, C = unused.
    // Root references A; the result must keep A and B (in registry order) and
    // drop C.
    let graph = registry(vec![
        proj_def(
            "A",
            SchemaType::record(vec![proj_field("b", SchemaType::ref_to(TypeId::new("B")))]),
        ),
        proj_def(
            "B",
            SchemaType::record(vec![proj_field("s", SchemaType::string())]),
        ),
        proj_def("C", SchemaType::u32()),
    ]);
    let input_schema = InputSchema::parameters([NamedField::user_supplied(
        "a",
        SchemaType::ref_to(TypeId::new("A")),
    )]);
    // record { a: A { b: B { s: "x" } } }
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::Record {
            fields: vec![SchemaValue::Record {
                fields: vec![SchemaValue::String("x".to_string())],
            }],
        }],
    })
    .unwrap();

    let typed = json_input_schema_value_to_typed_schema_value(json, &graph, &input_schema).unwrap();

    assert_eq!(proj_ids(&typed), vec!["A".to_string(), "B".to_string()]);
}

#[test]
fn json_input_handles_recursive_defs_without_looping() {
    // Node = record { value: u64, next: option<Ref(Node)> }.
    let graph = registry(vec![proj_def(
        "Node",
        SchemaType::record(vec![
            proj_field("value", SchemaType::u64()),
            proj_field(
                "next",
                SchemaType::option(SchemaType::ref_to(TypeId::new("Node"))),
            ),
        ]),
    )]);
    let input_schema = InputSchema::parameters([NamedField::user_supplied(
        "n",
        SchemaType::ref_to(TypeId::new("Node")),
    )]);
    // record { n: Node { value: 1, next: Some(Node { value: 2, next: None }) } }
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::Record {
            fields: vec![
                SchemaValue::U64(1),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::Record {
                        fields: vec![SchemaValue::U64(2), SchemaValue::Option { inner: None }],
                    })),
                },
            ],
        }],
    })
    .unwrap();

    let typed = json_input_schema_value_to_typed_schema_value(json, &graph, &input_schema).unwrap();

    assert_eq!(proj_ids(&typed), vec!["Node".to_string()]);
}

#[test]
fn json_input_projects_all_schema_alternatives_not_value_branch() {
    // A variant whose two cases reference different defs. The value only uses
    // the first case, but projection follows *schema* reachability, so both
    // defs must survive.
    let graph = registry(vec![
        proj_def(
            "First",
            SchemaType::record(vec![proj_field("x", SchemaType::u32())]),
        ),
        proj_def(
            "Second",
            SchemaType::record(vec![proj_field("y", SchemaType::string())]),
        ),
    ]);
    let variant = SchemaType::variant(vec![
        VariantCaseType {
            name: "first".to_string(),
            payload: Some(SchemaType::ref_to(TypeId::new("First"))),
            metadata: MetadataEnvelope::default(),
        },
        VariantCaseType {
            name: "second".to_string(),
            payload: Some(SchemaType::ref_to(TypeId::new("Second"))),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let input_schema = InputSchema::parameters([NamedField::user_supplied("choice", variant)]);
    // record { choice: variant#0(First { x: 1 }) }
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::Variant(VariantValuePayload {
            case: 0,
            payload: Some(Box::new(SchemaValue::Record {
                fields: vec![SchemaValue::U32(1)],
            })),
        })],
    })
    .unwrap();

    let typed = json_input_schema_value_to_typed_schema_value(json, &graph, &input_schema).unwrap();

    let mut ids = proj_ids(&typed);
    ids.sort();
    assert_eq!(ids, vec!["First".to_string(), "Second".to_string()]);
}

#[test]
fn json_input_dangling_ref_reports_error_not_panic() {
    // Root references a def absent from the registry; validation must surface a
    // dangling-ref error string rather than panicking.
    let graph = registry(vec![]);
    let input_schema = InputSchema::parameters([NamedField::user_supplied(
        "x",
        SchemaType::ref_to(TypeId::new("Missing")),
    )]);
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::U32(1)],
    })
    .unwrap();

    let err = json_input_schema_value_to_typed_schema_value(json, &graph, &input_schema)
        .expect_err("dangling ref must fail validation");

    assert!(
        err.contains("Missing"),
        "expected dangling-ref error mentioning the missing id, got: {err}"
    );
}

#[test]
fn json_input_projected_result_round_trips_and_revalidates() {
    // The projected (smaller) typed value must still be self-contained: it
    // round-trips through serde and re-validates against its own graph.
    let graph = registry(vec![
        proj_def(
            "Keep",
            SchemaType::record(vec![proj_field("s", SchemaType::string())]),
        ),
        proj_def("Drop", SchemaType::u32()),
    ]);
    let input_schema = InputSchema::parameters([NamedField::user_supplied(
        "k",
        SchemaType::ref_to(TypeId::new("Keep")),
    )]);
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::Record {
            fields: vec![SchemaValue::String("hi".to_string())],
        }],
    })
    .unwrap();

    let typed = json_input_schema_value_to_typed_schema_value(json, &graph, &input_schema).unwrap();
    assert_eq!(proj_ids(&typed), vec!["Keep".to_string()]);

    let encoded = serde_json::to_string(&typed).unwrap();
    let decoded: TypedSchemaValue = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, typed);

    crate::schema::validation::value::validate_value(
        decoded.graph(),
        decoded.root_type(),
        decoded.value(),
    )
    .expect("projected value must still validate against its own graph");
}

#[test]
fn json_input_with_auto_injected_field_validates_caller_only_record() {
    // A method/constructor input schema that mixes user-supplied fields with an
    // auto-injected `principal` field. The REST caller supplies only the
    // user-supplied values (the host fills the principal out of band), so the
    // caller-only record must validate, and the synthesized self-contained root
    // must describe exactly the user-supplied fields.
    let input_schema = InputSchema::parameters([
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("label", SchemaType::string()),
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        ),
    ]);
    // Caller supplies only the two user-supplied values.
    let json = serde_json::to_value(SchemaValue::Record {
        fields: vec![SchemaValue::U32(7), SchemaValue::String("hi".to_string())],
    })
    .unwrap();

    let typed =
        json_input_schema_value_to_typed_schema_value(json, &SchemaGraph::empty(), &input_schema)
            .expect("caller-only record (excluding auto-injected fields) must validate");

    let SchemaType::Record { fields, .. } = typed.root_type() else {
        panic!("expected record root, got {:?}", typed.root_type());
    };
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["count", "label"]);
}

// --- typed_schema_value_with_projected_defs (A4/A5/agent_config site 2) ---

#[test]
fn projected_helper_keeps_only_reachable_defs_and_sets_root() {
    // Registry with one reachable def (transitively) and one unreachable def.
    // The helper must keep `A` and `B` (in registry order), drop `C`, set the
    // given `root`, and carry the value verbatim (it is already validated).
    let graph = registry(vec![
        proj_def(
            "A",
            SchemaType::record(vec![proj_field("b", SchemaType::ref_to(TypeId::new("B")))]),
        ),
        proj_def(
            "B",
            SchemaType::record(vec![proj_field("s", SchemaType::string())]),
        ),
        proj_def("C", SchemaType::u32()),
    ]);
    let root = SchemaType::record(vec![proj_field("a", SchemaType::ref_to(TypeId::new("A")))]);
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::Record {
            fields: vec![SchemaValue::Record {
                fields: vec![SchemaValue::String("hi".to_string())],
            }],
        }],
    };

    let typed = typed_schema_value_with_projected_defs(&graph, root.clone(), value.clone());

    assert_eq!(proj_ids(&typed), vec!["A".to_string(), "B".to_string()]);
    assert_eq!(typed.root_type(), &root);
    assert_eq!(typed.value(), &value);

    // The projected carrier is self-contained: it validates against its own graph.
    crate::schema::validation::value::validate_value(
        typed.graph(),
        typed.root_type(),
        typed.value(),
    )
    .expect("projected carrier must validate against its own graph");
}

#[test]
fn projected_helper_drops_all_defs_for_ref_free_root() {
    // A ref-free root references no defs, so the projected graph is empty.
    let graph = registry(vec![proj_def("Unused", SchemaType::string())]);
    let root = SchemaType::record(vec![proj_field("n", SchemaType::u64())]);
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::U64(7)],
    };

    let typed = typed_schema_value_with_projected_defs(&graph, root, value);

    assert!(typed.graph().defs.is_empty());
}
