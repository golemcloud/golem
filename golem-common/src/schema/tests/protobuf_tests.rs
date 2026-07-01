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
use crate::base_model::account::AccountId;
use crate::base_model::agent::{
    AgentMode, AgentTypeName, RegisteredAgentTypeImplementer, Snapshotting,
};
use crate::base_model::component::{ComponentId, ComponentRevision};
use crate::model::account::AccountEmail;
use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
    AutoInjectedKind, FieldSource, InputSchema, NamedField, OutputSchema,
    RegisteredAgentTypeSchema,
};
use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::{MetadataEnvelope, Role, TypeId};
use crate::schema::proptest_strategies as strategies;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use proptest::prelude::*;
use strategies::{
    schema_graph_strategy, schema_value_strategy, schema_values_eq, typed_schema_value_strategy,
};
use test_r::test;
use uuid::Uuid;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Converting any well-formed schema graph to its protobuf mirror and back
    /// yields the original graph (recursive equality). Covers every
    /// `SchemaType` variant, all rich-scalar specs, metadata, and the union /
    /// discriminator types.
    #[test]
    fn schema_graph_proto_round_trip(graph in schema_graph_strategy()) {
        let proto: golem_api_grpc::proto::golem::schema::SchemaGraph = graph.clone().into();
        let back: SchemaGraph = proto.try_into().expect("decode");
        prop_assert_eq!(graph, back);
    }

    /// Converting any schema value to its protobuf mirror and back yields the
    /// original value (NaN-tolerant). Covers every `SchemaValue` variant,
    /// including the recursive/boxed ones (variant / option / result / union /
    /// map) and the rich leaves (text / binary / datetime / duration /
    /// quantity / secret / quota-token).
    #[test]
    fn schema_value_proto_round_trip(value in schema_value_strategy()) {
        let proto: golem_api_grpc::proto::golem::schema::SchemaValue = value.clone().into();
        let back: SchemaValue = proto.try_into().expect("decode");
        prop_assert!(
            schema_values_eq(&value, &back),
            "value round-trip mismatch:\n  before: {value:?}\n  after:  {back:?}"
        );
    }

    /// The typed pair (graph + value) round-trips through its protobuf mirror.
    #[test]
    fn typed_schema_value_proto_round_trip(typed in typed_schema_value_strategy()) {
        let proto: golem_api_grpc::proto::golem::schema::TypedSchemaValue = typed.clone().into();
        let back: TypedSchemaValue = proto.try_into().expect("decode");
        prop_assert_eq!(typed.graph(), back.graph());
        prop_assert!(
            schema_values_eq(typed.value(), back.value()),
            "typed value round-trip mismatch:\n  before: {typed:?}\n  after:  {back:?}"
        );
    }
}

fn sample_agent_type_schema() -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: AgentTypeName("weather-agent".to_string()),
        description: "A weather agent".to_string(),
        source_language: "rust".to_string(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: TypeId::new("city"),
                name: Some("City".to_string()),
                body: SchemaType::record(vec![]),
            }],
            root: SchemaType::record(vec![]),
        },
        constructor: AgentConstructorSchema {
            name: Some("new".to_string()),
            description: "constructs".to_string(),
            prompt_hint: Some("hint".to_string()),
            input_schema: InputSchema::parameters([
                NamedField::user_supplied("location", SchemaType::ref_to(TypeId::new("city"))),
                NamedField::auto_injected(
                    "principal",
                    AutoInjectedKind::Principal,
                    SchemaType::string(),
                ),
            ]),
        },
        methods: vec![AgentMethodSchema {
            name: "forecast".to_string(),
            description: "gets the forecast".to_string(),
            prompt_hint: None,
            input_schema: InputSchema::parameters([NamedField::user_supplied(
                "days",
                SchemaType::u32(),
            )]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::string().with_metadata(
                MetadataEnvelope {
                    role: Some(Role::Multimodal),
                    ..Default::default()
                },
            ))),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![AgentDependencySchema {
            type_name: "geocoder".to_string(),
            description: Some("resolves coordinates".to_string()),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: "dep ctor".to_string(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
            },
            methods: vec![AgentMethodSchema {
                name: "resolve".to_string(),
                description: "resolves".to_string(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([NamedField::user_supplied(
                    "name",
                    SchemaType::string(),
                )]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            }],
        }],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    }
}

#[test]
fn agent_type_schema_proto_round_trip() {
    let schema = sample_agent_type_schema();
    let proto: golem_api_grpc::proto::golem::schema::AgentTypeSchema = schema.clone().into();
    let back: AgentTypeSchema = proto.try_into().expect("decode");
    assert_eq!(schema, back);
}

#[test]
fn registered_agent_type_schema_proto_round_trip() {
    let registered = RegisteredAgentTypeSchema {
        agent_type: sample_agent_type_schema(),
        implemented_by: RegisteredAgentTypeImplementer {
            component_id: ComponentId(Uuid::nil()),
            component_revision: ComponentRevision::INITIAL,
            component_name: "test-component".to_string(),
            account_id: AccountId(Uuid::from_u128(0x1234)),
            account_email: AccountEmail::new("test@golem.cloud"),
        },
    };
    let proto: golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema =
        registered.clone().into();
    let back: RegisteredAgentTypeSchema = proto.try_into().expect("decode");
    assert_eq!(registered, back);
}

#[test]
fn field_source_round_trips() {
    for source in [
        FieldSource::UserSupplied,
        FieldSource::AutoInjected(AutoInjectedKind::Principal),
    ] {
        let proto: golem_api_grpc::proto::golem::schema::FieldSource = source.clone().into();
        let back: FieldSource = proto.try_into().expect("decode");
        assert_eq!(source, back);
    }
}

/// Deterministic numeric-restriction vectors round-trip through the protobuf
/// mirror exactly.
#[test]
fn numeric_restrictions_proto_golden_round_trip() {
    for (label, ty) in crate::schema::tests::golden_numeric_schema_types() {
        let graph = SchemaGraph::anonymous(ty);
        let proto: golem_api_grpc::proto::golem::schema::SchemaGraph = graph.clone().into();
        let back: SchemaGraph = proto.try_into().expect("decode");
        assert_eq!(graph, back, "proto numeric golden mismatch: {label}");
    }
}

/// A stored `Some(empty)` numeric restriction normalizes to `None` when it
/// crosses the protobuf decode boundary (covers both `unit: None` and the
/// empty-string `unit` case).
#[test]
fn numeric_empty_restrictions_normalize_to_none_proto() {
    use crate::schema::schema_type::{NumericRestrictions, SchemaType};

    for empty in [
        NumericRestrictions::default(),
        NumericRestrictions {
            min: None,
            max: None,
            unit: Some(String::new()),
        },
    ] {
        let graph = SchemaGraph::anonymous(SchemaType::U32 {
            restrictions: Some(empty),
            metadata: MetadataEnvelope::default(),
        });
        let proto: golem_api_grpc::proto::golem::schema::SchemaGraph = graph.into();
        let back: SchemaGraph = proto.try_into().expect("decode");
        assert_eq!(back.root.numeric_restrictions(), None);
    }
}
