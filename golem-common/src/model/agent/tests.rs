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
use crate::base_model::agent::{
    AgentMode, AgentTypeName, Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation,
    SnapshottingPeriodic,
};
use crate::model::agent::{AgentTypeSchemaResolver, ParsedAgentId};
use crate::schema::{
    AgentConstructorSchema, AgentTypeSchema, BinaryRestrictions, InputSchema, MetadataEnvelope,
    NamedField, NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TextRestrictions,
    TypedSchemaValue,
};
use crate::{agent_id, data_value, phantom_agent_id};
use poem_openapi::types::ToJSON;
use pretty_assertions::assert_eq;
use proptest::{prop_assert_eq, proptest};
use std::collections::HashMap;
use test_r::test;
use uuid::Uuid;
#[test]
fn agent_id_structural_normalization() {
    {
        let agent_id =
            ParsedAgentId::parse("agent-7(  [  12,     13 , 14 ]   )", TestAgentTypes::new())
                .unwrap();
        assert_eq!(agent_id.to_string(), "agent-7([12,13,14])");
    }

    {
        // Structural format: record is positional `(x,y,flags)`, flags are `f(indices...)`
        let agent_id = ParsedAgentId::parse(
            "agent-3(  32 ,( 12, 32, f(0,    1  , 2   ) ))",
            TestAgentTypes::new(),
        )
        .unwrap();
        assert_eq!(agent_id.to_string(), "agent-3(32,(12,32,f(0,1,2)))");
    }
}

#[test]
fn invalid_phantom_id() {
    failure_test_with_string(
        "agent-1()[not-a-uuid]",
        "Invalid UUID in phantom ID: invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `n` at 1",
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

// Tests for AgentId::normalize_text

#[test]
fn normalize_strips_whitespace_in_wave_values() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-7(  [  12,     13 , 14 ]   )").unwrap(),
        "agent-7([12,13,14])"
    );
}

#[test]
fn normalize_strips_whitespace_in_records() {
    assert_eq!(
        ParsedAgentId::normalize_text(
            r#"agent-3(  32 ,{ x  : 12, y: 32, properties: {a,    b  , c   } })"#
        )
        .unwrap(),
        "agent-3(32,{x:12,y:32,properties:{a,b,c}})"
    );
}

#[test]
fn normalize_preserves_already_compact() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-1()").unwrap(),
        "agent-1()"
    );
}

#[test]
fn normalize_preserves_strings() {
    assert_eq!(
        ParsedAgentId::normalize_text(r#"agent-2("hello world")"#).unwrap(),
        r#"agent-2("hello world")"#
    );
}

#[test]
fn normalize_handles_phantom_id() {
    let result =
        ParsedAgentId::normalize_text("agent-1()[550e8400-e29b-41d4-a716-446655440000]").unwrap();
    assert_eq!(result, "agent-1()[550e8400-e29b-41d4-a716-446655440000]");
}

#[test]
fn normalize_handles_phantom_id_with_whitespace() {
    let result =
        ParsedAgentId::normalize_text("agent-1()[ 550e8400-e29b-41d4-a716-446655440000 ]").unwrap();
    assert_eq!(result, "agent-1()[550e8400-e29b-41d4-a716-446655440000]");
}

#[test]
fn normalize_rejects_invalid_format() {
    assert!(ParsedAgentId::normalize_text("not-an-agent-id").is_err());
}

#[test]
fn normalize_rejects_invalid_phantom_id() {
    assert!(ParsedAgentId::normalize_text("agent-1()[not-a-uuid]").is_err());
}

#[test]
fn normalize_handles_urls() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-4(https://url1.com/,https://url2.com/)").unwrap(),
        "agent-4(https://url1.com/,https://url2.com/)"
    );
}

#[test]
fn normalize_handles_inline_text() {
    assert_eq!(
        ParsedAgentId::normalize_text(r#"agent-4("hello, world!","goodbye")"#).unwrap(),
        r#"agent-4("hello, world!","goodbye")"#
    );
}

#[test]
fn normalize_handles_multimodal_elements() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-6(x(  42  ),y(https://example.com/))").unwrap(),
        "agent-6(x(42),y(https://example.com/))"
    );
}

#[test]
fn normalize_handles_nested_records_with_whitespace() {
    assert_eq!(
        ParsedAgentId::normalize_text(
            r#"non-kebab-agent({ agent-id : { component-id : { uuid : { high-bits : 115746831381919841 , low-bits : 13556493125794766855 } } , agent-id : "some-agent-id(\"hello\")" } , oplog-idx : 1234 })"#
        )
        .unwrap(),
        r#"non-kebab-agent({agent-id:{component-id:{uuid:{high-bits:115746831381919841,low-bits:13556493125794766855}},agent-id:"some-agent-id(\"hello\")"},oplog-idx:1234})"#
    );
}

#[test]
fn normalize_handles_options_and_results() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( some( 42 ) )").unwrap(),
        "agent-x(some(42))"
    );
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( none )").unwrap(),
        "agent-x(none)"
    );
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( ok( 1 ) )").unwrap(),
        "agent-x(ok(1))"
    );
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( err( 2 ) )").unwrap(),
        "agent-x(err(2))"
    );
}

#[test]
fn normalize_handles_empty_record() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( {  :  } )").unwrap(),
        "agent-x({:})"
    );
}

#[test]
fn normalize_handles_empty_flags() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( {  } )").unwrap(),
        "agent-x({})"
    );
}

#[test]
fn normalize_handles_char_values() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( 'a' , 'b' )").unwrap(),
        "agent-x('a','b')"
    );
}

#[test]
fn normalize_handles_variant_with_percent_prefix() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x( %true( 42 ) )").unwrap(),
        "agent-x(%true(42))"
    );
}

#[test]
fn normalize_trims_outer_whitespace() {
    assert_eq!(
        ParsedAgentId::normalize_text("  agent-7(  [  12, 13 ]  )  ").unwrap(),
        "agent-7([12,13])"
    );
}

#[test]
fn normalize_phantom_id_with_casing_and_whitespace() {
    let result =
        ParsedAgentId::normalize_text("agent-1(  )[ 550E8400-E29B-41D4-A716-446655440000 ]")
            .unwrap();
    assert_eq!(result, "agent-1()[550e8400-e29b-41d4-a716-446655440000]");
}

#[test]
fn normalize_empty_params_stays_empty() {
    assert_eq!(
        ParsedAgentId::normalize_text("agent-1(   )").unwrap(),
        "agent-1()"
    );
}

#[test]
fn normalize_preserves_double_comma() {
    // normalize_structural only strips whitespace, it does not validate structure
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x(1,,2)").unwrap(),
        "agent-x(1,,2)"
    );
}

#[test]
fn normalize_preserves_leading_comma() {
    // normalize_structural only strips whitespace, it does not validate structure
    assert_eq!(
        ParsedAgentId::normalize_text("agent-x(,1)").unwrap(),
        "agent-x(,1)"
    );
}

#[test]
fn normalize_rejects_empty_agent_type() {
    assert!(ParsedAgentId::normalize_text("()").is_err());
}

proptest! {
    #[test]
    fn normalize_text_idempotent_for_simple_agent(x in 0u32..10000) {
        let agent_id = ParsedAgentId::try_new(
            AgentTypeName("agent-2".to_string()),
            data_value!(x),
            None,
        ).unwrap();
        let canonical = agent_id.to_string();
        let normalized = ParsedAgentId::normalize_text(&canonical).unwrap();
        prop_assert_eq!(&normalized, &canonical);
    }

    #[test]
    fn normalize_text_idempotent_for_list_agent(
        a in 0u32..100,
        b in 0u32..100,
        c in 0u32..100,
    ) {
        let agent_id = ParsedAgentId::try_new(
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
            None,
        ).unwrap();
        let canonical = agent_id.to_string();
        let normalized = ParsedAgentId::normalize_text(&canonical).unwrap();
        prop_assert_eq!(&normalized, &canonical);
    }

    #[test]
    fn normalize_text_strips_whitespace_for_simple_agent(x in 0u32..10000) {
        let with_spaces = format!("agent-2(  {x}  )");
        let normalized = ParsedAgentId::normalize_text(&with_spaces).unwrap();
        prop_assert_eq!(normalized, format!("agent-2({x})"));
    }

    #[test]
    fn normalize_text_strips_whitespace_for_list(
        a in 0u32..100,
        b in 0u32..100,
        c in 0u32..100,
    ) {
        let with_spaces = format!("agent-7( [ {a} , {b} , {c} ] )");
        let normalized = ParsedAgentId::normalize_text(&with_spaces).unwrap();
        prop_assert_eq!(normalized, format!("agent-7([{a},{b},{c}])"));
    }
}

fn failure_test_with_string(agent_id_str: &str, expected_failure: &str) {
    let id2 = ParsedAgentId::parse(agent_id_str, TestAgentTypes::new())
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
fn agent_id_macro_no_parameters() {
    let id = agent_id!("agent-1");
    assert_eq!(id.agent_type, AgentTypeName("agent-1".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record { fields: vec![] }
    );
    assert_eq!(id.phantom_id, None);
}

#[test]
fn agent_id_macro_single_parameter() {
    let id = agent_id!("agent-2", 42u32);
    assert_eq!(id.agent_type, AgentTypeName("agent-2".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42)]
        }
    );
    assert_eq!(id.phantom_id, None);
}

#[test]
fn agent_id_macro_multiple_parameters() {
    let id = agent_id!("agent-3", 42u32, 100u64, 3u8);
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
    assert_eq!(id.phantom_id, None);
}

#[test]
fn agent_id_macro_with_trailing_comma() {
    let id = agent_id!("agent-4", 42u32, 100u64,);
    assert_eq!(id.agent_type, AgentTypeName("agent-4".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42), SchemaValue::U64(100)]
        }
    );
    assert_eq!(id.phantom_id, None);
}

#[test]
fn phantom_agent_id_macro_no_parameters() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-1", phantom_uuid);
    assert_eq!(id.agent_type, AgentTypeName("phantom-1".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record { fields: vec![] }
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn phantom_agent_id_macro_single_parameter() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-2", phantom_uuid, 42u32);
    assert_eq!(id.agent_type, AgentTypeName("phantom-2".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42)]
        }
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn phantom_agent_id_macro_multiple_parameters() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-3", phantom_uuid, 42u32, 100u64);
    assert_eq!(id.agent_type, AgentTypeName("phantom-3".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42), SchemaValue::U64(100)]
        }
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn phantom_agent_id_macro_with_trailing_comma() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-4", phantom_uuid, 42u32, 100u64,);
    assert_eq!(id.agent_type, AgentTypeName("phantom-4".to_string()));
    assert_eq!(
        id.parameters.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(42), SchemaValue::U64(100)]
        }
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn agent_id_vs_phantom_agent_id() {
    let uuid = Uuid::now_v7();
    let regular_id = agent_id!("test", 42u32);
    let phantom_id = phantom_agent_id!("test", uuid, 42u32);

    // Both should have the same type name and parameters
    assert_eq!(regular_id.agent_type, phantom_id.agent_type);
    assert_eq!(regular_id.parameters, phantom_id.parameters);

    // But different phantom_id values
    assert_eq!(regular_id.phantom_id, None);
    assert_eq!(phantom_id.phantom_id, Some(uuid));
}

#[test]
fn agent_id_too_long_rejected() {
    use crate::base_model::AgentId;
    use crate::base_model::component::ComponentId;

    let component_id = ComponentId(Uuid::nil());
    let parameters = data_value!("a".repeat(600));

    let err = ParsedAgentId::try_new(AgentTypeName("t".to_string()), parameters.clone(), None)
        .expect_err("ParsedAgentId::new should reject too-long ids");
    assert!(
        err.contains("too long"),
        "Error should mention 'too long', got: {err}"
    );

    let parsed = ParsedAgentId::new(AgentTypeName("t".to_string()), parameters, None);
    let result = AgentId::from_agent_id(component_id, &parsed);
    assert!(result.is_err(), "Expected error for too-long agent ID");
    let err = result.unwrap_err();
    assert!(
        err.contains("too long"),
        "Error should mention 'too long', got: {err}"
    );
}

#[test]
fn auto_phantom_lenient_allows_too_long_id_but_agent_id_still_rejects() {
    use crate::base_model::AgentId;
    use crate::base_model::component::ComponentId;

    let component_id = ComponentId(Uuid::nil());
    let agent_type = AgentTypeName("t".to_string());
    let params = data_value!("a".repeat(600));

    let err = ParsedAgentId::new_auto_phantom(
        agent_type.clone(),
        params.clone(),
        None,
        AgentMode::Durable,
    )
    .expect_err("ParsedAgentId::new_auto_phantom should reject too-long ids");
    assert!(
        err.contains("too long"),
        "Error should mention 'too long', got: {err}"
    );

    let parsed = ParsedAgentId::new(agent_type, params, None);

    let result = AgentId::from_agent_id(component_id, &parsed);
    assert!(result.is_err(), "Expected error for too-long agent ID");
    let err = result.unwrap_err();
    assert!(
        err.contains("too long"),
        "Error should mention 'too long', got: {err}"
    );
}

#[test]
fn agent_id_at_max_length_accepted() {
    use crate::base_model::AgentId;
    use crate::base_model::component::ComponentId;

    let component_id = ComponentId(Uuid::nil());
    // Format is: t("aaa...aaa") → 1 + 1 + 1 + N + 1 + 1 = N + 5, so N = 507 for total 512
    let parsed = ParsedAgentId::try_new(
        AgentTypeName("t".to_string()),
        data_value!("a".repeat(507)),
        None,
    )
    .expect("ParsedAgentId::new should succeed");
    assert_eq!(
        parsed.to_string().len(),
        512,
        "ParsedAgentId string should be exactly 512 chars"
    );
    let result = AgentId::from_agent_id(component_id, &parsed);
    assert!(
        result.is_ok(),
        "Expected success for exactly 512-char agent ID, got: {:?}",
        result
    );
}

#[test]
fn new_auto_phantom_durable_none() {
    let agent_type = AgentTypeName("test-agent".to_string());
    let id = ParsedAgentId::new_auto_phantom(agent_type, data_value!(), None, AgentMode::Durable)
        .unwrap();
    assert_eq!(
        id.phantom_id, None,
        "Durable agent with no phantom_id should remain None"
    );
}

#[test]
fn new_auto_phantom_durable_some() {
    let supplied = Uuid::new_v4();
    let agent_type = AgentTypeName("test-agent".to_string());
    let id = ParsedAgentId::new_auto_phantom(
        agent_type,
        data_value!(),
        Some(supplied),
        AgentMode::Durable,
    )
    .unwrap();
    assert_eq!(
        id.phantom_id,
        Some(supplied),
        "Durable agent should preserve supplied phantom_id"
    );
}

#[test]
fn new_auto_phantom_ephemeral_none() {
    let agent_type = AgentTypeName("test-agent".to_string());
    let id = ParsedAgentId::new_auto_phantom(agent_type, data_value!(), None, AgentMode::Ephemeral)
        .unwrap();
    assert!(
        id.phantom_id.is_some(),
        "Ephemeral agent with no phantom_id should auto-generate one"
    );
    // Verify it appears in the string representation
    let s = id.to_string();
    assert!(
        s.contains('['),
        "Ephemeral auto-phantom ID should appear in string: {s}"
    );
}

#[test]
fn new_auto_phantom_ephemeral_some() {
    let supplied = Uuid::new_v4();
    let agent_type = AgentTypeName("test-agent".to_string());
    let id = ParsedAgentId::new_auto_phantom(
        agent_type,
        data_value!(),
        Some(supplied),
        AgentMode::Ephemeral,
    )
    .unwrap();
    assert_eq!(
        id.phantom_id,
        Some(supplied),
        "Ephemeral agent should preserve supplied phantom_id"
    );
}

mod read_only_config_roundtrip {
    use crate::base_model::Empty;
    use crate::base_model::agent::{CachePolicy, CachePolicyTtl, ReadOnlyConfig};
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
            for uses_principal in [false, true] {
                let cfg = ReadOnlyConfig {
                    cache_policy: cache_policy.clone(),
                    uses_principal,
                };
                let proto: golem_api_grpc::proto::golem::component::ReadOnlyConfig =
                    cfg.clone().into();
                let back: ReadOnlyConfig = proto.try_into().expect("protobuf decode");
                assert_eq!(cfg, back);
            }
        }
    }

    #[test]
    fn read_only_config_protobuf_missing_cache_policy_errors() {
        let proto = golem_api_grpc::proto::golem::component::ReadOnlyConfig {
            cache_policy: None,
            uses_principal: false,
        };
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
                    secret_ref: "shhh".to_string(),
                }),
            ],
        };
        let typed = TypedSchemaValue::new(graph, value);
        let err = AgentError::CustomError(typed);
        let rendered = err.to_string();
        assert!(
            rendered.contains("<redacted>"),
            "expected secret to be redacted, got: {rendered:?}"
        );
        assert!(
            !rendered.contains("shhh"),
            "secret ref must not leak into Display, got: {rendered:?}"
        );
    }
}
