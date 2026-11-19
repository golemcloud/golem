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

use crate::base_model::agent::{
    AgentMode, AgentTypeName, Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation,
    SnapshottingPeriodic,
};
use crate::base_model::{AgentId, RESERVED_TYPE_TAG};
use crate::model::agent::{
    AgentTypeSchemaResolver, InvocationFreshnessDisposition, OwnerKind, ParsedAgentKey,
    ResolvedOwnerContext,
};
use golem_schema::Empty;

#[test]
fn owner_context_distinguishes_guest_workers_from_host_only_owners() {
    use crate::model::component_metadata::{ComponentMetadata, KnownExports};

    let non_agent = ComponentMetadata::default();
    let agent = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        test_agent_types().into_values().collect(),
        Default::default(),
    );
    let wrong_export = ComponentMetadata::from_parts(
        KnownExports {
            save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
            ..Default::default()
        },
        vec![],
        vec![],
        Default::default(),
    );
    let typed_id = AgentId::from_parts(7, 1);
    let reserved_id = AgentId::random_reserved();
    let key = || ParsedAgentKey::parse("agent-2(1)", &agent).unwrap();

    for agent_types in [vec![], test_agent_types().into_values().collect()] {
        let processor = ComponentMetadata::from_parts(
            KnownExports {
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
                ..Default::default()
            },
            vec![],
            agent_types,
            Default::default(),
        );
        let resolved = ResolvedOwnerContext::from_authoritative_kind(
            OwnerKind::OplogProcessor,
            &reserved_id,
            None,
            &processor,
        )
        .unwrap();
        assert_eq!(resolved, ResolvedOwnerContext::OplogProcessor);
        assert!(resolved.agent().is_none());
        assert!(
            processor
                .owner_plugins(OwnerKind::OplogProcessor, None)
                .is_none()
        );
        assert!(
            processor
                .owner_plugins(
                    OwnerKind::OplogProcessor,
                    Some(&AgentTypeName("agent-2".to_string()))
                )
                .is_none()
        );
        assert!(
            ResolvedOwnerContext::from_authoritative_kind(
                OwnerKind::ComponentAgent,
                &reserved_id,
                Some(key()),
                &processor,
            )
            .is_err()
        );
    }

    assert!(
        ResolvedOwnerContext::from_authoritative_kind(
            OwnerKind::ComponentAgent,
            &typed_id,
            Some(key()),
            &non_agent,
        )
        .is_err(),
    );
    let typed = ResolvedOwnerContext::from_authoritative_kind(
        OwnerKind::ComponentAgent,
        &typed_id,
        Some(key()),
        &agent,
    )
    .unwrap();
    assert_eq!(typed.agent().unwrap().to_string(), "agent-2(1)");
    assert!(
        ResolvedOwnerContext::from_authoritative_kind(
            OwnerKind::ComponentAgent,
            &typed_id,
            None,
            &agent,
        )
        .is_err()
    );

    for metadata in [&non_agent, &agent, &wrong_export] {
        assert!(
            ResolvedOwnerContext::from_authoritative_kind(
                OwnerKind::OplogProcessor,
                &reserved_id,
                None,
                metadata,
            )
            .is_err()
        );
        assert_eq!(
            ResolvedOwnerContext::from_authoritative_kind(
                OwnerKind::EphemeralExternalTool,
                &reserved_id,
                None,
                metadata,
            )
            .unwrap(),
            ResolvedOwnerContext::ComponentBaseline,
        );
        assert!(
            ResolvedOwnerContext::from_authoritative_kind(
                OwnerKind::ComponentAgent,
                &reserved_id,
                Some(key()),
                metadata,
            )
            .is_err()
        );
        assert!(
            ResolvedOwnerContext::from_authoritative_kind(
                OwnerKind::EphemeralExternalTool,
                &typed_id,
                None,
                metadata,
            )
            .is_err()
        );
    }
}

#[test]
fn only_component_agents_have_a_type_tag() {
    let typed = AgentId::from_parts(7, 1);
    let reserved = AgentId::from_parts(RESERVED_TYPE_TAG, 1);
    assert!(OwnerKind::ComponentAgent.validate_agent_id(&typed).is_ok());
    assert!(
        OwnerKind::ComponentAgent
            .validate_agent_id(&reserved)
            .is_err()
    );
    for kind in [OwnerKind::EphemeralExternalTool, OwnerKind::OplogProcessor] {
        assert!(kind.validate_agent_id(&reserved).is_ok());
        assert!(kind.validate_agent_id(&typed).is_err());
    }
    let version_4: AgentId = "00000007-0000-4000-8000-000000000001".parse().unwrap();
    let version_4_reserved: AgentId = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    assert!(
        OwnerKind::ComponentAgent
            .validate_agent_id(&version_4)
            .is_err()
    );
    assert!(
        OwnerKind::OplogProcessor
            .validate_agent_id(&version_4_reserved)
            .is_err()
    );
    for (kind, discriminant, json) in [
        (OwnerKind::ComponentAgent, 0, "\"component-agent\""),
        (
            OwnerKind::EphemeralExternalTool,
            1,
            "\"ephemeral-external-tool\"",
        ),
        (OwnerKind::OplogProcessor, 2, "\"oplog-processor\""),
    ] {
        assert_eq!(i32::from(kind), discriminant);
        assert_eq!(OwnerKind::try_from(discriminant).unwrap(), kind);
        assert_eq!(serde_json::to_string(&kind).unwrap(), json);
        assert_eq!(serde_json::from_str::<OwnerKind>(json).unwrap(), kind);
    }
    for discriminant in [-1, 3, i32::MAX] {
        assert!(OwnerKind::try_from(discriminant).is_err());
    }
}
use crate::schema::{
    AgentConstructorSchema, AgentTypeSchema, BinaryRestrictions, InputSchema, MetadataEnvelope,
    NamedField, NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TextRestrictions,
    TypedSchemaValue,
};
use crate::{agent_key, data_value};
use poem_openapi::types::ToJSON;
use pretty_assertions::assert_eq;
use proptest::{prop_assert_eq, proptest};
use std::collections::HashMap;
use test_r::test;
#[test]
fn agent_id_structural_normalization() {
    {
        let agent_id =
            ParsedAgentKey::parse("agent-7(  [  12,     13 , 14 ]   )", TestAgentTypes::new())
                .unwrap();
        assert_eq!(agent_id.to_string(), "agent-7([12,13,14])");
    }

    {
        // Structural format: record is positional `(x,y,flags)`, flags are `f(indices...)`
        let agent_id = ParsedAgentKey::parse(
            "agent-3(  32 ,( 12, 32, f(0,    1  , 2   ) ))",
            TestAgentTypes::new(),
        )
        .unwrap();
        assert_eq!(agent_id.to_string(), "agent-3(32,(12,32,f(0,1,2)))");
    }
}

#[test]
fn a_key_longer_than_512_bytes_is_rejected() {
    // `t("…")` is the string plus five bytes; `é` is two bytes, so the limit counts bytes.
    let at_limit = format!("{}é", "a".repeat(505));
    let key = super::AgentKey::new(AgentTypeName("t".to_string()), data_value!(at_limit))
        .expect("a 512-byte key is accepted");
    assert_eq!(key.to_string().len(), 512);

    let over_limit = format!("{}é", "a".repeat(506));
    let err = super::AgentKey::new(AgentTypeName("t".to_string()), data_value!(over_limit))
        .expect_err("a 513-byte key is rejected");
    assert_eq!(
        err.to_string(),
        "Agent key is too long: 513 bytes, max length: 512"
    );

    // `agent-7([…])` around 300 one-digit elements is 610 bytes.
    let err = ParsedAgentKey::parse(
        format!("agent-7([{}])", vec!["1"; 300].join(",")),
        TestAgentTypes::new(),
    )
    .expect_err("parsed key text is limited too");
    assert_eq!(err, "Agent key is too long: 610 bytes, max length: 512");
}

#[test]
fn constructor_roundtrip_has_no_key_length_limit() {
    let constructor =
        ParsedAgentKey::try_new(AgentTypeName("Doc".into()), data_value!("é".repeat(520))).unwrap();
    let proto =
        golem_api_grpc::proto::golem::worker::AgentKey::try_from(constructor.clone()).unwrap();
    assert_eq!(
        ParsedAgentKey::try_from(proto.clone()).unwrap(),
        constructor
    );
    assert!(
        super::AgentKey::try_from(proto)
            .unwrap_err()
            .contains("Agent key is too long")
    );
    let env = crate::model::environment::EnvironmentId::new();
    let type_id = crate::model::type_id::TypeId::try_from(42u32).unwrap();
    let logical_id = AgentId::ephemeral_proxy(env, type_id, &constructor).unwrap();
    assert_eq!(
        logical_id,
        AgentId::ephemeral_proxy(env, type_id, &constructor).unwrap()
    );
    let other =
        ParsedAgentKey::try_new(AgentTypeName("Doc".into()), data_value!("é".repeat(521))).unwrap();
    assert_ne!(
        logical_id,
        AgentId::ephemeral_proxy(env, type_id, &other).unwrap()
    );
}

#[test]
fn a_phantom_suffix_is_not_part_of_a_key() {
    failure_test_with_string(
        "agent-1()[550e8400-e29b-41d4-a716-446655440000]",
        "Unexpected agent key format - must be 'agent-type(...)', got: agent-1()[550e8400-e29b-41d4-a716-446655440000]",
    )
}

fn snapshotting_serde_poem_roundtrip(original: Snapshotting) {
    let poem_serialized = original.to_json_string();
    let serde_serialized = serde_json::to_string(&original).unwrap();

    let poem_json: serde_json::Value = serde_json::from_str(&poem_serialized).unwrap();
    let serde_json: serde_json::Value = serde_json::from_str(&serde_serialized).unwrap();
    assert_eq!(poem_json, serde_json);

    let from_poem: Snapshotting = serde_json::from_str(&poem_serialized).unwrap();
    let from_serde: Snapshotting = serde_json::from_str(&serde_serialized).unwrap();
    assert_eq!(original, from_poem);
    assert_eq!(original, from_serde);
}

#[test]
fn snapshotting_disabled_serde_poem_roundtrip() {
    snapshotting_serde_poem_roundtrip(Snapshotting::Disabled(Empty {}));
}

#[test]
fn snapshotting_enabled_default_serde_poem_roundtrip() {
    snapshotting_serde_poem_roundtrip(Snapshotting::Enabled(SnapshottingConfig::Default(Empty {})));
}

#[test]
fn snapshotting_enabled_periodic_serde_poem_roundtrip() {
    snapshotting_serde_poem_roundtrip(Snapshotting::Enabled(SnapshottingConfig::Periodic(
        SnapshottingPeriodic {
            duration_nanos: 2_000_000_000,
        },
    )));
}

#[test]
fn snapshotting_enabled_every_n_invocation_serde_poem_roundtrip() {
    snapshotting_serde_poem_roundtrip(Snapshotting::Enabled(SnapshottingConfig::EveryNInvocation(
        SnapshottingEveryNInvocation { count: 5 },
    )));
}

// Tests for ParsedAgentKey::normalize_text

#[test]
fn normalize_strips_whitespace_in_wave_values() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-7(  [  12,     13 , 14 ]   )").unwrap(),
        "agent-7([12,13,14])"
    );
}

#[test]
fn normalize_strips_whitespace_in_records() {
    assert_eq!(
        ParsedAgentKey::normalize_text(
            r#"agent-3(  32 ,{ x  : 12, y: 32, properties: {a,    b  , c   } })"#
        )
        .unwrap(),
        "agent-3(32,{x:12,y:32,properties:{a,b,c}})"
    );
}

#[test]
fn normalize_preserves_already_compact() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-1()").unwrap(),
        "agent-1()"
    );
}

#[test]
fn normalize_preserves_strings() {
    assert_eq!(
        ParsedAgentKey::normalize_text(r#"agent-2("hello world")"#).unwrap(),
        r#"agent-2("hello world")"#
    );
}

#[test]
fn normalize_rejects_a_phantom_suffix() {
    assert!(
        ParsedAgentKey::normalize_text("agent-1()[550e8400-e29b-41d4-a716-446655440000]").is_err()
    );
}

#[test]
fn normalize_rejects_invalid_format() {
    assert!(ParsedAgentKey::normalize_text("not-an-agent-id").is_err());
}

#[test]
fn normalize_handles_urls() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-4(https://url1.com/,https://url2.com/)").unwrap(),
        "agent-4(https://url1.com/,https://url2.com/)"
    );
}

#[test]
fn normalize_handles_inline_text() {
    assert_eq!(
        ParsedAgentKey::normalize_text(r#"agent-4("hello, world!","goodbye")"#).unwrap(),
        r#"agent-4("hello, world!","goodbye")"#
    );
}

#[test]
fn normalize_handles_multimodal_elements() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-6(x(  42  ),y(https://example.com/))").unwrap(),
        "agent-6(x(42),y(https://example.com/))"
    );
}

#[test]
fn normalize_handles_nested_records_with_whitespace() {
    assert_eq!(
        ParsedAgentKey::normalize_text(
            r#"non-kebab-agent({ agent-id : { component-id : { uuid : { high-bits : 115746831381919841 , low-bits : 13556493125794766855 } } , agent-id : "some-agent-id(\"hello\")" } , oplog-idx : 1234 })"#
        )
        .unwrap(),
        r#"non-kebab-agent({agent-id:{component-id:{uuid:{high-bits:115746831381919841,low-bits:13556493125794766855}},agent-id:"some-agent-id(\"hello\")"},oplog-idx:1234})"#
    );
}

#[test]
fn normalize_handles_options_and_results() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( some( 42 ) )").unwrap(),
        "agent-x(some(42))"
    );
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( none )").unwrap(),
        "agent-x(none)"
    );
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( ok( 1 ) )").unwrap(),
        "agent-x(ok(1))"
    );
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( err( 2 ) )").unwrap(),
        "agent-x(err(2))"
    );
}

#[test]
fn normalize_handles_empty_record() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( {  :  } )").unwrap(),
        "agent-x({:})"
    );
}

#[test]
fn normalize_handles_empty_flags() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( {  } )").unwrap(),
        "agent-x({})"
    );
}

#[test]
fn normalize_handles_char_values() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( 'a' , 'b' )").unwrap(),
        "agent-x('a','b')"
    );
}

#[test]
fn normalize_handles_variant_with_percent_prefix() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x( %true( 42 ) )").unwrap(),
        "agent-x(%true(42))"
    );
}

#[test]
fn normalize_trims_outer_whitespace() {
    assert_eq!(
        ParsedAgentKey::normalize_text("  agent-7(  [  12, 13 ]  )  ").unwrap(),
        "agent-7([12,13])"
    );
}

#[test]
fn normalize_empty_params_stays_empty() {
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-1(   )").unwrap(),
        "agent-1()"
    );
}

#[test]
fn normalize_preserves_double_comma() {
    // normalize_structural only strips whitespace, it does not validate structure
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x(1,,2)").unwrap(),
        "agent-x(1,,2)"
    );
}

#[test]
fn normalize_preserves_leading_comma() {
    // normalize_structural only strips whitespace, it does not validate structure
    assert_eq!(
        ParsedAgentKey::normalize_text("agent-x(,1)").unwrap(),
        "agent-x(,1)"
    );
}

#[test]
fn normalize_rejects_empty_agent_type() {
    assert!(ParsedAgentKey::normalize_text("()").is_err());
}

proptest! {
    #[test]
    fn normalize_text_idempotent_for_simple_agent(x in 0u32..10000) {
        let agent_id = ParsedAgentKey::try_new(
            AgentTypeName("agent-2".to_string()),
            data_value!(x),
        ).unwrap();
        let canonical = agent_id.to_string();
        let normalized = ParsedAgentKey::normalize_text(&canonical).unwrap();
        prop_assert_eq!(&normalized, &canonical);
    }

    #[test]
    fn normalize_text_idempotent_for_list_agent(
        a in 0u32..100,
        b in 0u32..100,
        c in 0u32..100,
    ) {
        let agent_id = ParsedAgentKey::try_new(
            AgentTypeName("agent-7".to_string()),
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(vec![NamedFieldType {
                    name: "args".into(),
                    body: SchemaType::list(SchemaType::u32()),
                    metadata: Default::default(),
                }])),
                SchemaValue::Record {
                    fields: vec![SchemaValue::List {
                        elements: vec![SchemaValue::U32(a), SchemaValue::U32(b), SchemaValue::U32(c)],
                    }],
                },
            ),

        ).unwrap();
        let canonical = agent_id.to_string();
        let normalized = ParsedAgentKey::normalize_text(&canonical).unwrap();
        prop_assert_eq!(&normalized, &canonical);
    }

    #[test]
    fn normalize_text_strips_whitespace_for_simple_agent(x in 0u32..10000) {
        let with_spaces = format!("agent-2(  {x}  )");
        let normalized = ParsedAgentKey::normalize_text(&with_spaces).unwrap();
        prop_assert_eq!(normalized, format!("agent-2({x})"));
    }

    #[test]
    fn normalize_text_strips_whitespace_for_list(
        a in 0u32..100,
        b in 0u32..100,
        c in 0u32..100,
    ) {
        let with_spaces = format!("agent-7( [ {a} , {b} , {c} ] )");
        let normalized = ParsedAgentKey::normalize_text(&with_spaces).unwrap();
        prop_assert_eq!(normalized, format!("agent-7([{a},{b},{c}])"));
    }
}

fn failure_test_with_string(agent_id_str: &str, expected_failure: &str) {
    let id2 = ParsedAgentKey::parse(agent_id_str, TestAgentTypes::new())
        .err()
        .unwrap();
    assert_eq!(id2, expected_failure.to_string());
}

struct TestAgentTypes {
    types: HashMap<AgentTypeName, AgentTypeSchema>,
}

impl TestAgentTypes {
    pub fn new() -> Self {
        Self {
            types: test_agent_types(),
        }
    }
}

impl AgentTypeSchemaResolver for TestAgentTypes {
    fn resolve_agent_type_schema_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentTypeSchema, String> {
        self.types
            .get(agent_type)
            .cloned()
            .ok_or_else(|| format!("Unknown agent type: {}", agent_type))
    }
}

fn field(name: &str, body: SchemaType) -> NamedField {
    NamedField {
        name: name.to_string(),
        source: Default::default(),
        schema: body,
        metadata: MetadataEnvelope::default(),
    }
}

fn record_field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: Default::default(),
    }
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

fn test_agent_types() -> HashMap<AgentTypeName, AgentTypeSchema> {
    let agent_types = [
        make_agent_type("agent-1", vec![]),
        make_agent_type("agent-2", vec![("x", SchemaType::u32())]),
        make_agent_type(
            "agent-3",
            vec![
                ("x", SchemaType::u32()),
                (
                    "r",
                    SchemaType::record(vec![
                        record_field("x", SchemaType::u32()),
                        record_field("y", SchemaType::u32()),
                        record_field(
                            "properties",
                            SchemaType::flags(vec![
                                "a".to_string(),
                                "b".to_string(),
                                "c".to_string(),
                            ]),
                        ),
                    ]),
                ),
            ],
        ),
        make_agent_type(
            "agent-4",
            vec![
                ("a", SchemaType::text(TextRestrictions::default())),
                ("b", SchemaType::text(TextRestrictions::default())),
            ],
        ),
        make_agent_type(
            "agent-5",
            vec![
                ("a", SchemaType::binary(BinaryRestrictions::default())),
                ("b", SchemaType::binary(BinaryRestrictions::default())),
            ],
        ),
        make_agent_type(
            "agent-7",
            vec![("args", SchemaType::list(SchemaType::u32()))],
        ),
    ];

    agent_types
        .into_iter()
        .map(|agent_type| (agent_type.type_name.clone(), agent_type))
        .collect()
}

#[test]
fn data_value_macro_empty() {
    let value = data_value!();
    assert_eq!(
        value.value(),
        &crate::schema::SchemaValue::Record { fields: vec![] }
    );
}

#[test]
fn data_value_macro_single_u32() {
    let value = data_value!(42u32);
    assert_eq!(
        value.value(),
        &crate::schema::SchemaValue::Record {
            fields: vec![crate::schema::SchemaValue::U32(42)]
        }
    );
}

#[test]
fn data_value_macro_multiple_primitives() {
    let value = data_value!(42u32, 100u64, 3u8);
    assert_eq!(
        value.value(),
        &crate::schema::SchemaValue::Record {
            fields: vec![
                crate::schema::SchemaValue::U32(42),
                crate::schema::SchemaValue::U64(100),
                crate::schema::SchemaValue::U8(3),
            ]
        }
    );
}

#[test]
fn data_value_macro_mixed_types() {
    let value = data_value!(42u32, 3u8);
    assert_eq!(
        value.value(),
        &crate::schema::SchemaValue::Record {
            fields: vec![
                crate::schema::SchemaValue::U32(42),
                crate::schema::SchemaValue::U8(3),
            ]
        }
    );
}

#[test]
fn data_value_macro_trailing_comma() {
    let value = data_value!(42u32, 100u64,);
    assert_eq!(
        value.value(),
        &crate::schema::SchemaValue::Record {
            fields: vec![
                crate::schema::SchemaValue::U32(42),
                crate::schema::SchemaValue::U64(100),
            ]
        }
    );
}

#[test]
fn agent_key_macro_no_parameters() {
    let id = agent_key!("agent-1");
    assert_eq!(id.agent_type, AgentTypeName("agent-1".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record { fields: vec![] }
    );
}

#[test]
fn agent_key_macro_single_parameter() {
    let id = agent_key!("agent-2", 42u32);
    assert_eq!(id.agent_type, AgentTypeName("agent-2".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42)]
        }
    );
}

#[test]
fn agent_key_macro_multiple_parameters() {
    let id = agent_key!("agent-3", 42u32, 100u64, 3u8);
    assert_eq!(id.agent_type, AgentTypeName("agent-3".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(42),
                SchemaValue::U64(100),
                SchemaValue::U8(3)
            ]
        }
    );
}

#[test]
fn agent_key_macro_with_trailing_comma() {
    let id = agent_key!("agent-4", 42u32, 100u64,);
    assert_eq!(id.agent_type, AgentTypeName("agent-4".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42), SchemaValue::U64(100)]
        }
    );
}

#[test]
fn invocation_freshness_defaults_to_may_exist() {
    assert_eq!(
        InvocationFreshnessDisposition::default(),
        InvocationFreshnessDisposition::MayExist
    );
}

mod read_only_config_roundtrip {
    use crate::base_model::agent::{CachePolicy, CachePolicyTtl, ReadOnlyConfig};
    use golem_schema::Empty;
    use pretty_assertions::assert_eq;
    use test_r::test;

    fn all_cache_policies() -> Vec<CachePolicy> {
        vec![
            CachePolicy::NoCache(Empty {}),
            CachePolicy::UntilWrite(Empty {}),
            CachePolicy::Ttl(CachePolicyTtl {
                duration_nanos: 1_234_567_890,
            }),
        ]
    }

    #[test]
    fn cache_policy_protobuf_roundtrip() {
        for policy in all_cache_policies() {
            let proto: golem_api_grpc::proto::golem::component::CachePolicy = policy.clone().into();
            let back: CachePolicy = proto.try_into().expect("protobuf decode");
            assert_eq!(policy, back);
        }
    }

    #[test]
    fn read_only_config_protobuf_roundtrip() {
        for cache_policy in all_cache_policies() {
            let cfg = ReadOnlyConfig {
                cache_policy: cache_policy.clone(),
            };
            let proto: golem_api_grpc::proto::golem::component::ReadOnlyConfig = cfg.clone().into();
            let back: ReadOnlyConfig = proto.try_into().expect("protobuf decode");
            assert_eq!(cfg, back);
        }
    }

    #[test]
    fn read_only_config_protobuf_missing_cache_policy_errors() {
        let proto = golem_api_grpc::proto::golem::component::ReadOnlyConfig { cache_policy: None };
        let result: Result<ReadOnlyConfig, _> = proto.try_into();
        assert!(result.is_err());
    }
}

mod agent_error_tests {
    use crate::model::agent::AgentError;
    use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
    use crate::schema::schema_type::{NamedFieldType, SchemaType, SecretSpec, TextRestrictions};
    use crate::schema::schema_value::{SchemaValue, SecretValuePayload, TextValuePayload};
    use pretty_assertions::assert_eq;
    use test_r::test;

    #[test]
    fn display_renders_primitive_payload_exact() {
        let typed = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::u32()),
            SchemaValue::U32(7),
        );
        let err = AgentError::CustomError(typed);
        assert_eq!(err.to_string(), "7");
    }

    #[test]
    fn display_renders_record_payload_exact() {
        let graph = SchemaGraph::anonymous(SchemaType::record(vec![
            NamedFieldType {
                name: "id".to_string(),
                body: SchemaType::u32(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "name".to_string(),
                body: SchemaType::text(TextRestrictions::default()),
                metadata: Default::default(),
            },
        ]));
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(7),
                SchemaValue::Text(TextValuePayload {
                    text: "Ada".to_string(),
                    language: None,
                }),
            ],
        };
        let err = AgentError::CustomError(TypedSchemaValue::new(graph, value));
        assert_eq!(err.to_string(), "{ id: 7, name: Ada }");
    }

    #[test]
    fn display_redacts_secret_payload() {
        // `Display` is user-facing/loggable; the renderer must redact
        // capability nodes such as `Secret` so accidental embedding of a
        // secret in a custom error doesn't leak it into logs.
        let graph = SchemaGraph::anonymous(SchemaType::record(vec![
            NamedFieldType {
                name: "label".to_string(),
                body: SchemaType::text(TextRestrictions::default()),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "token".to_string(),
                body: SchemaType::secret(SecretSpec::default()),
                metadata: Default::default(),
            },
        ]));
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::Text(TextValuePayload {
                    text: "auth".to_string(),
                    language: None,
                }),
                SchemaValue::Secret(SecretValuePayload {
                    secret_id: uuid::Uuid::nil(),
                    config_key: None,
                    version: 0,
                    resolved_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                    category: None,
                }),
            ],
        };
        let typed = TypedSchemaValue::new(graph, value);
        let err = AgentError::CustomError(typed);
        let rendered = err.to_string();
        assert!(
            rendered.contains("<redacted: secret>"),
            "expected secret to be redacted, got: {rendered:?}"
        );
        assert!(
            !rendered.contains("shhh"),
            "secret ref must not leak into Display, got: {rendered:?}"
        );
    }
}

#[test]
fn recorded_constructor_parameters_with_delimiters_round_trip() {
    use crate::model::account::{AccountEmail, AccountId};
    use crate::model::agent::{AgentMetadata, AgentStatusRecord};
    use crate::model::caller::Caller;
    use crate::model::component::ComponentId;
    use crate::model::component_metadata::{ComponentMetadata, KnownExports};
    use crate::model::environment::EnvironmentId;
    use crate::model::invocation_context::InvocationContextStack;
    use crate::model::oplog::{CreateRequest, CreateRequestKind};
    use crate::model::{AgentFingerprint, Timestamp};
    use uuid::Uuid;

    let component_metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        test_agent_types().into_values().collect(),
        Default::default(),
    );
    let key =
        ParsedAgentKey::parse(r#"agent-4(@t"a(b), c)", @t")(,")"#, &component_metadata).unwrap();
    let parameters = key.canonical_parameters().unwrap();
    assert_eq!(parameters, r#"@t"a(b), c)",@t")(,""#);

    let metadata = AgentMetadata {
        component_id: ComponentId(Uuid::from_u128(1)),
        agent_type_name: Some(key.agent_type.clone()),
        canonical_key: Some(key.to_string()),
        constructor_parameters: Some(parameters.clone()),
        agent_id: AgentId::from_parts(7, 1),
        owner_kind: OwnerKind::ComponentAgent,
        env: vec![],
        environment_id: EnvironmentId(Uuid::from_u128(2)),
        created_by: AccountId(Uuid::from_u128(3)),
        created_by_email: AccountEmail::new("test@golem"),
        config: vec![],
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord::default(),
        fingerprint: AgentFingerprint(Uuid::from_u128(4)),
        agent_mode: AgentMode::Durable,
        create_request: CreateRequest::new(
            CreateRequestKind::GetOrCreate,
            None,
            Caller::anonymous(),
            &InvocationContextStack::fresh_rounded(),
        ),
    };
    let parsed = metadata.parse_agent_id(&component_metadata).unwrap();
    assert_eq!(parsed, key);
    assert_eq!(parsed.to_string(), format!("agent-4({parameters})"));

    let without_parameters = AgentMetadata {
        constructor_parameters: None,
        ..metadata.clone()
    };
    assert!(
        without_parameters
            .parse_agent_id(&component_metadata)
            .is_err()
    );
    let without_type = AgentMetadata {
        agent_type_name: None,
        ..metadata
    };
    assert!(without_type.parse_agent_id(&component_metadata).is_err());
}

#[test]
fn agent_targets_round_trip_through_protobuf() {
    use crate::model::agent::{AgentKey, AgentTarget};
    use crate::model::component_metadata::{ComponentMetadata, KnownExports};
    use golem_api_grpc::proto::golem::worker as proto;

    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![],
        test_agent_types().into_values().collect(),
        Default::default(),
    );
    let key = AgentKey::parse("agent-3(7,(1,2,f(0,2)))", &metadata)
        .unwrap()
        .0;
    for target in [
        AgentTarget::Key(key.clone()),
        AgentTarget::Id(AgentId::from_parts(7, 9)),
    ] {
        let wire = proto::AgentTarget::try_from(target.clone()).unwrap();
        assert_eq!(AgentTarget::try_from(wire).unwrap(), target);
    }

    let wire = proto::AgentKey::try_from(key.clone()).unwrap();
    assert_eq!(wire.agent_type_name, "agent-3");
    let decoded = AgentKey::try_from(wire.clone()).unwrap();
    assert_eq!(decoded.canonical_params(), key.canonical_params());
    assert_eq!(decoded.parameters(), key.parameters());
    assert!(
        AgentKey::try_from(proto::AgentKey {
            parameters: None,
            ..wire
        })
        .is_err()
    );
    assert!(AgentTarget::try_from(proto::AgentTarget { target: None }).is_err());
}

#[test]
fn type_id_kinds_round_trip_through_protobuf() {
    use crate::model::type_id::TypeIdKind;
    use golem_api_grpc::proto::golem::registry::v1::TypeIdKind as Proto;

    for (kind, wire) in [
        (TypeIdKind::AgentType, Proto::AgentType),
        (TypeIdKind::Tool, Proto::Tool),
    ] {
        assert_eq!(Proto::from(kind), wire);
        assert_eq!(TypeIdKind::try_from(wire), Ok(kind));
    }
    assert!(TypeIdKind::try_from(Proto::Unspecified).is_err());
    assert_eq!(Proto::default(), Proto::Unspecified);
}

#[test]
fn storage_keys_round_trip_and_share_a_prefix_per_type() {
    use crate::model::OwnedAgentId;
    use crate::model::environment::EnvironmentId;
    use crate::model::type_id::TypeId;
    use uuid::Uuid;

    let environment_id = EnvironmentId(Uuid::from_u128(0xabcd));
    let agent = OwnedAgentId::new(environment_id, &AgentId::from_parts(0x0000_01a2, 5));
    let key = agent.storage_key();
    assert_eq!(
        key,
        format!("00000000-0000-0000-0000-00000000abcd/{}", agent.agent_id)
    );
    assert_eq!(OwnedAgentId::from_storage_key(&key), Some(agent.clone()));

    let prefix = OwnedAgentId::type_storage_key_prefix(
        environment_id,
        TypeId::try_from(0x0000_01a2_u32).unwrap(),
    );
    assert_eq!(prefix, "00000000-0000-0000-0000-00000000abcd/000001a2-");
    assert!(key.starts_with(&prefix));
    let other_type = OwnedAgentId::new(environment_id, &AgentId::from_parts(0x0000_01a3, 5));
    assert!(!other_type.storage_key().starts_with(&prefix));
    let other_environment = OwnedAgentId::new(EnvironmentId(Uuid::from_u128(1)), &agent.agent_id);
    assert!(!other_environment.storage_key().starts_with(&prefix));

    for not_a_key in [
        key.to_uppercase(),
        format!("{}/{{{}}}", environment_id, agent.agent_id),
        format!("{environment_id}:{}", agent.agent_id),
        format!("{key}#stage"),
        agent.agent_id.to_string(),
    ] {
        assert_eq!(
            OwnedAgentId::from_storage_key(&not_a_key),
            None,
            "{not_a_key}"
        );
    }
}

mod caller_protobuf_tests {
    use crate::base_model::caller::test_support::{
        every_principal, original_claims_json, sample_caller,
    };
    use crate::base_model::caller::{Caller, Pair, PrincipalRef, TokenProvenance};
    use golem_api_grpc::proto::golem::component as proto;
    use test_r::test;

    #[test]
    fn caller_protobuf_round_trips_every_kind() {
        let mut callers = vec![sample_caller()];
        for original in every_principal() {
            for immediate in every_principal() {
                callers.push(Caller {
                    original: original.clone(),
                    immediate,
                    tokens: vec![
                        TokenProvenance {
                            id: uuid::Uuid::from_u128(1),
                            maker: Some(PrincipalRef::new(Pair::new("https://i.example", "s"))),
                        },
                        TokenProvenance {
                            id: uuid::Uuid::from_u128(2),
                            maker: None,
                        },
                    ],
                });
            }
        }
        for caller in callers {
            let wire: proto::Caller = caller.clone().into();
            let bytes = prost::Message::encode_to_vec(&wire);
            let wire: proto::Caller = prost::Message::decode(bytes.as_slice()).unwrap();
            let back = Caller::try_from(wire).expect("decode");
            assert_eq!(back, caller);
        }
    }

    #[test]
    fn caller_protobuf_keeps_the_claims_text_byte_for_byte() {
        let caller = sample_caller();
        let wire: proto::Caller = caller.clone().into();
        let back = Caller::try_from(wire).unwrap();
        assert_eq!(original_claims_json(&back), original_claims_json(&caller));
    }

    #[test]
    fn caller_protobuf_writes_equal_roles_once() {
        use crate::base_model::caller::test_support::{agent_principal, every_principal};

        for principal in every_principal() {
            let same: proto::Caller = Caller::entry(principal.clone()).into();
            assert!(same.immediate.is_none(), "{principal:?}");
            assert_eq!(Caller::try_from(same).unwrap(), Caller::entry(principal));
        }

        let different = Caller {
            original: every_principal().remove(0),
            immediate: agent_principal(),
            tokens: vec![],
        };
        let wire: proto::Caller = different.clone().into();
        assert!(wire.immediate.is_some());
        assert_eq!(Caller::try_from(wire).unwrap(), different);

        let spelled = proto::Caller {
            original: Some(every_principal().remove(0).into()),
            immediate: Some(every_principal().remove(0).into()),
            tokens: vec![],
        };
        assert!(Caller::try_from(spelled).is_ok());
    }

    #[test]
    fn caller_protobuf_refuses_a_token_without_an_id() {
        let mut wire: proto::Caller = Caller {
            tokens: vec![TokenProvenance {
                id: uuid::Uuid::from_u128(1),
                maker: None,
            }],
            ..sample_caller()
        }
        .into();
        wire.tokens[0].id = None;
        let error = Caller::try_from(wire).unwrap_err();
        assert!(error.contains("id"), "{error}");
    }

    #[test]
    fn caller_protobuf_refuses_malformed_callers() {
        let valid: proto::Caller = sample_caller().into();

        let mut without_original = valid.clone();
        without_original.original = None;
        assert!(Caller::try_from(without_original).is_err());

        let mut without_immediate = valid.clone();
        without_immediate.immediate = None;
        let decoded = Caller::try_from(without_immediate).unwrap();
        assert!(decoded.immediate.same_record(&decoded.original));

        let mut bad_claims = valid.clone();
        let Some(proto::principal::Value::User(user)) =
            bad_claims.original.as_mut().and_then(|p| p.value.as_mut())
        else {
            panic!("the sample caller starts with a user");
        };
        user.claims = "[1,2]".to_string();
        assert!(Caller::try_from(bad_claims).is_err());

        let account = proto::Principal {
            value: Some(proto::principal::Value::Account(proto::AccountPrincipal {
                issuer: "https://golem.example".to_string(),
                account_id: Some(golem_api_grpc::proto::golem::common::AccountId {
                    value: Some(golem_schema::proto::golem::common::Uuid::from(
                        uuid::Uuid::from_u128(5),
                    )),
                }),
                authentication: proto::AccountAuthentication::Unspecified as i32,
            })),
        };
        let unspecified = proto::Caller {
            original: Some(account.clone()),
            immediate: Some(account),
            tokens: vec![],
        };
        assert!(Caller::try_from(unspecified).is_err());

        let empty_principal = proto::Caller {
            original: Some(proto::Principal { value: None }),
            immediate: valid.immediate,
            tokens: vec![],
        };
        assert!(Caller::try_from(empty_principal).is_err());
    }
}

mod caller_authenticated_at_tests {
    use crate::base_model::caller::Caller;
    use crate::base_model::caller::test_support::sample_caller;
    use golem_api_grpc::proto::golem::component as proto;
    use test_r::test;

    fn with_authenticated_at(seconds: i64, nanos: i32) -> proto::Caller {
        let mut wire: proto::Caller = sample_caller().into();
        let Some(proto::principal::Value::User(user)) = wire
            .original
            .as_mut()
            .and_then(|principal| principal.value.as_mut())
        else {
            panic!("the sample caller starts with a user");
        };
        user.authenticated_at = Some(prost_types::Timestamp { seconds, nanos });
        wire
    }

    #[test]
    fn an_out_of_range_authenticated_at_is_refused_not_a_panic() {
        for (seconds, nanos) in [
            (-1, 0),
            (0, -1),
            (0, 1_000_000_000),
            (i64::MAX, 0),
            (253_402_300_800, 0),
        ] {
            let result = Caller::try_from(with_authenticated_at(seconds, nanos));
            assert!(result.is_err(), "{seconds}/{nanos} decoded: {result:?}");
        }
        assert!(Caller::try_from(with_authenticated_at(253_402_300_799, 999_999_999)).is_ok());
    }

    #[test]
    fn a_sub_millisecond_authenticated_at_survives_the_oplog_codec() {
        let caller = Caller::try_from(with_authenticated_at(1_767_323_045, 678_123_456)).unwrap();
        let bytes = desert_rust::serialize_to_byte_vec(&caller).unwrap();
        let recorded: Caller = desert_rust::deserialize(&bytes).unwrap();
        assert_eq!(recorded, caller, "the recorded caller is another record");
    }
}
