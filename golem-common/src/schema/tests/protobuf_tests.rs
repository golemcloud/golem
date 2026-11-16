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
    AgentMode, AgentTypeName, CorsOptions, DurableStreamInputSlotSource,
    DurableStreamOutputSlotSource, DurableStreamRouteLoadOptions, DurableStreamRouteOptions,
    DurableStreamSlotOptions, DurableStreamSlotSource, HttpEndpointDetails, HttpMethod,
    RegisteredAgentTypeImplementer, Snapshotting,
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
use crate::schema::schema_type::{PermissionCardSpec, SchemaType};
use crate::schema::schema_value::{PermissionCardValuePayload, SchemaValue};
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
        let proto: golem_schema::proto::golem::schema::SchemaGraph = graph.clone().into();
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
        let proto: golem_schema::proto::golem::schema::SchemaValue =
            value.clone().try_into().expect("encode");
        let back: SchemaValue = proto.try_into().expect("decode");
        prop_assert!(
            schema_values_eq(&value, &back),
            "value round-trip mismatch:\n  before: {value:?}\n  after:  {back:?}"
        );
    }

    /// The typed pair (graph + value) round-trips through its protobuf mirror.
    #[test]
    fn typed_schema_value_proto_round_trip(typed in typed_schema_value_strategy()) {
        let proto: golem_schema::proto::golem::schema::TypedSchemaValue =
            typed.clone().try_into().expect("encode");
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
        kind: crate::schema::agent::AgentTypeKind::Regular,
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
            http_endpoint: vec![HttpEndpointDetails {
                http_method: HttpMethod::Post(Empty {}),
                path_suffix: vec![],
                header_vars: vec![],
                query_vars: vec![],
                auth_details: None,
                cors_options: CorsOptions {
                    allowed_patterns: vec![],
                },
                mode: Some(golem_schema::http::HttpRouteMode::Sse),
                durable_streams: Some(DurableStreamRouteOptions {
                    slots: vec![
                        DurableStreamSlotOptions {
                            source: DurableStreamSlotSource::Input(DurableStreamInputSlotSource {
                                name: "events".to_string(),
                            }),
                            name: Some("messages".to_string()),
                            content_type: None,
                        },
                        DurableStreamSlotOptions {
                            source: DurableStreamSlotSource::Output(
                                DurableStreamOutputSlotSource {
                                    name: "$result".to_string(),
                                },
                            ),
                            name: None,
                            content_type: Some("application/vnd.golem.events".to_string()),
                        },
                    ],
                    load: Some(DurableStreamRouteLoadOptions {
                        max_concurrent_readers_per_stream: Some(8),
                        max_append_requests_per_second_per_stream: None,
                    }),
                }),
            }],
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
    use golem_schema::http::HttpRouteMode;
    for mode in [
        None,
        Some(HttpRouteMode::DurableStreams),
        Some(HttpRouteMode::Plain),
        Some(HttpRouteMode::Sse),
        Some(HttpRouteMode::WebSocket),
    ] {
        let mut schema = sample_agent_type_schema();
        schema.methods[0].http_endpoint[0].mode = mode;
        let proto: golem_api_grpc::proto::golem::schema::AgentTypeSchema = schema.clone().into();
        let back: AgentTypeSchema = proto.try_into().expect("decode");
        assert_eq!(schema, back);
    }
}

fn sample_registered_agent_type_schema() -> RegisteredAgentTypeSchema {
    RegisteredAgentTypeSchema {
        agent_type: sample_agent_type_schema(),
        implemented_by: RegisteredAgentTypeImplementer {
            component_id: ComponentId(Uuid::nil()),
            component_revision: ComponentRevision::INITIAL,
            component_name: "test-component".to_string(),
            account_id: AccountId(Uuid::from_u128(0x1234)),
            account_email: AccountEmail::new("test@golem.cloud"),
        },
    }
}

#[test]
fn registered_agent_type_schema_proto_round_trip() {
    let registered = sample_registered_agent_type_schema();
    let proto: golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema =
        registered.clone().into();
    let back: RegisteredAgentTypeSchema = proto.try_into().expect("decode");
    assert_eq!(registered, back);
}

#[test]
fn registered_agent_type_proto_round_trip() {
    let schema = sample_registered_agent_type_schema();
    let registered = crate::model::agent::RegisteredAgentType {
        type_id: crate::model::type_id::TypeId::try_from(0x1234_5678u32).unwrap(),
        agent_type: schema.agent_type,
        implemented_by: schema.implemented_by,
    };
    let proto: golem_api_grpc::proto::golem::registry::RegisteredAgentType =
        registered.clone().into();
    assert_eq!(proto.type_id, 0x1234_5678);
    let mut invalid = proto.clone();
    invalid.type_id = 0;
    assert!(crate::model::agent::RegisteredAgentType::try_from(invalid).is_err());
    let back: crate::model::agent::RegisteredAgentType = proto.try_into().expect("decode");
    assert_eq!(registered, back);
}

#[test]
fn deployed_registered_agent_type_preserves_type_id() {
    let schema = sample_registered_agent_type_schema();
    let type_id = crate::model::type_id::TypeId::try_from(0x1234_5678u32).unwrap();
    let deployed = crate::model::agent::DeployedRegisteredAgentType {
        type_id,
        agent_type: schema.agent_type.clone(),
        implemented_by: schema.implemented_by.clone(),
        webhook_prefix_authority_and_path: Some("example.com/webhook".to_string()),
    };
    let registered: crate::model::agent::RegisteredAgentType = deployed.into();
    assert_eq!(registered.type_id, type_id);
    assert_eq!(registered.agent_type, schema.agent_type);
    assert_eq!(registered.implemented_by, schema.implemented_by);
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

#[test]
fn permission_card_type_and_value_proto_round_trip() {
    let graph = SchemaGraph::anonymous(SchemaType::permission_card(PermissionCardSpec {
        polymorphic: true,
    }));
    let graph_proto: golem_schema::proto::golem::schema::SchemaGraph = graph.clone().into();
    let graph_back: SchemaGraph = graph_proto.try_into().expect("decode permission-card type");
    assert_eq!(graph, graph_back);

    let value = SchemaValue::PermissionCard(PermissionCardValuePayload {
        card_id: Uuid::from_u128(1),
        parent_ids: vec![Uuid::from_u128(2), Uuid::from_u128(3)],
        expires_at: Some(chrono::DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap()),
        polymorphic: true,
    });
    let value_proto: golem_schema::proto::golem::schema::SchemaValue = value
        .clone()
        .try_into()
        .expect("encode permission-card value");
    let value_back: SchemaValue = value_proto
        .try_into()
        .expect("decode permission-card value");
    assert_eq!(value, value_back);
}

/// Deterministic numeric-restriction vectors round-trip through the protobuf
/// mirror exactly.
#[test]
fn numeric_restrictions_proto_golden_round_trip() {
    for (label, ty) in crate::schema::tests::golden_numeric_schema_types() {
        let graph = SchemaGraph::anonymous(ty);
        let proto: golem_schema::proto::golem::schema::SchemaGraph = graph.clone().into();
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
        let proto: golem_schema::proto::golem::schema::SchemaGraph = graph.into();
        let back: SchemaGraph = proto.try_into().expect("decode");
        assert_eq!(back.root.numeric_restrictions(), None);
    }
}
