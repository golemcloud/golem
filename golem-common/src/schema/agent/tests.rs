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
};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
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
