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

use crate::base_model::agent::{
    Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation, SnapshottingPeriodic,
};
use crate::base_model::Empty;
use crate::model::agent::{
    AgentConstructor, AgentId, AgentMode, AgentType, AgentTypeName, AgentTypeResolver,
    BinaryDescriptor, BinaryReference, BinarySource, BinaryType, ComponentModelElementSchema,
    DataSchema, DataValue, ElementSchema, ElementValue, ElementValues, JsonComponentModelValue,
    NamedElementSchema, NamedElementSchemas, NamedElementValue, NamedElementValues, TextDescriptor,
    TextReference, TextReferenceValue, TextSource, TextType, UntypedJsonDataValue,
    UntypedJsonElementValue, UntypedJsonElementValues, Url,
};
use crate::{agent_id, data_value, phantom_agent_id};
use async_trait::async_trait;
use golem_wasm::analysis::analysed_type::{field, flags, list, record, str, u32, u64};
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use poem_openapi::types::ToJSON;
use pretty_assertions::assert_eq;
use proptest::prelude::Strategy;
use proptest::strategy::Just;
use proptest::string::string_regex;
use proptest::{prop_assert_eq, prop_oneof, proptest};
use std::collections::HashMap;
use test_r::test;
use uuid::Uuid;

#[test]
fn agent_id_wave_normalization() {
    {
        let agent_id =
            AgentId::parse("agent-7(  [  12,     13 , 14 ]   )", TestAgentTypes::new()).unwrap();
        assert_eq!(agent_id.to_string(), "agent-7([12,13,14])");
    }

    {
        let agent_id = AgentId::parse(
            r#"agent-3(  32 ,{ x  : 12, y: 32, properties: {a,    b  , c   } })"#,
            TestAgentTypes::new(),
        )
        .unwrap();
        assert_eq!(
            agent_id.to_string(),
            "agent-3(32,{x:12,y:32,properties:{a,b,c}})"
        );
    }
}

#[test]
fn roundtrip_test_1() {
    roundtrip_test(
        "agent-1",
        DataValue::Tuple(ElementValues { elements: vec![] }),
    )
}

#[test]
fn roundtrip_test_2() {
    roundtrip_test(
        "agent-2",
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(12u32.into_value_and_type())],
        }),
    )
}

#[test]
fn roundtrip_test_3() {
    roundtrip_test(
        "agent-3",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(12u32.into_value_and_type()),
                ElementValue::ComponentModel(ValueAndType::new(
                    Value::Record(vec![
                        Value::U32(1),
                        Value::U32(2),
                        Value::Flags(vec![true, false, true]),
                    ]),
                    record(vec![
                        field("x", u32()),
                        field("y", u32()),
                        field("properties", flags(&["a", "b", "c"])),
                    ]),
                )),
            ],
        }),
    )
}

#[test]
fn roundtrip_test_4_1() {
    roundtrip_test(
        "agent-4",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::UnstructuredText(TextReference::Url(Url {
                    value: "https://url1.com/".to_string(),
                })),
                ElementValue::UnstructuredText(TextReference::Url(Url {
                    value: "https://url2.com/".to_string(),
                })),
            ],
        }),
    )
}

#[test]
fn roundtrip_test_4_2() {
    roundtrip_test(
        "agent-4",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::UnstructuredText(TextReference::Inline(TextSource {
                    data: "hello, world!".to_string(),
                    text_type: None,
                })),
                ElementValue::UnstructuredText(TextReference::Inline(TextSource {
                    data: "\\\"hello,\\\" world!".to_string(),
                    text_type: Some(TextType {
                        language_code: "en".to_string(),
                    }),
                })),
            ],
        }),
    )
}

fn text_type_strat() -> impl Strategy<Value = Option<TextType>> {
    prop_oneof! {
        Just(None),
        Just(Some(TextType { language_code: "en".to_string() })),
        Just(Some(TextType { language_code: "de".to_string() })),
        Just(Some(TextType { language_code: "hu".to_string() })),
    }
}

fn text_reference_strat() -> impl Strategy<Value = TextReference> {
    prop_oneof! {
        Just(TextReference::Url(Url { value: "https://example.com/xyz?a=1".to_string() })),
        (string_regex(".*\\p{Cc}.*").unwrap(), text_type_strat()).prop_map(|(data, text_type)| TextReference::Inline(TextSource {
            data,
            text_type
        }))
    }
}

proptest! {
    #[test]
    fn roundtrip_test_arbitrary_unstructured_text_in_multimodal(txt in text_reference_strat()) {
        let parameters = DataValue::Multimodal(
            NamedElementValues {
                elements: vec![
                    NamedElementValue {
                        name: "y".to_string(),
                        value: ElementValue::UnstructuredText(txt)
                    },
                ]
            }
        );
        let id = AgentId::new(AgentTypeName("agent-6".to_string()), parameters, None);
        let s = id.to_string();
        println!("{s}");
        let id2 = AgentId::parse(s, TestAgentTypes::new()).unwrap();
        prop_assert_eq!(id, id2);
    }

    #[test]
    fn roundtrip_test_multiple_arbitrary_unstructured_text_in_multimodal(txt1 in text_reference_strat(), txt2 in text_reference_strat()) {
        let parameters = DataValue::Multimodal(
            NamedElementValues {
                elements: vec![
                    NamedElementValue {
                        name: "y".to_string(),
                        value: ElementValue::UnstructuredText(txt1)
                    },
                    NamedElementValue {
                        name: "y".to_string(),
                        value: ElementValue::UnstructuredText(txt2)
                    },
                ]
            }
        );
        let id = AgentId::new(AgentTypeName("agent-6".to_string()), parameters, None);
        let s = id.to_string();
        println!("{s}");
        let id2 = AgentId::parse(s, TestAgentTypes::new()).unwrap();
        prop_assert_eq!(id, id2);
    }
}

#[test]
fn roundtrip_test_5_1() {
    roundtrip_test(
        "agent-5",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::UnstructuredBinary(BinaryReference::Url(Url {
                    value: "https://url1.com/".to_string(),
                })),
                ElementValue::UnstructuredBinary(BinaryReference::Url(Url {
                    value: "https://url2.com/".to_string(),
                })),
            ],
        }),
    )
}

#[test]
fn roundtrip_test_5_2() {
    roundtrip_test(
        "agent-5",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::UnstructuredBinary(BinaryReference::Inline(BinarySource {
                    data: "Hello world!".as_bytes().to_vec(),
                    binary_type: BinaryType {
                        mime_type: "application/json".to_string(),
                    },
                })),
                ElementValue::UnstructuredBinary(BinaryReference::Inline(BinarySource {
                    data: "Hello world!".as_bytes().to_vec(),
                    binary_type: BinaryType {
                        mime_type: "image/png".to_string(),
                    },
                })),
            ],
        }),
    )
}

#[test]
fn roundtrip_test_6() {
    roundtrip_test(
        "agent-6",
        DataValue::Multimodal(NamedElementValues {
            elements: vec![
                NamedElementValue {
                    name: "z".to_string(),
                    value: ElementValue::UnstructuredBinary(BinaryReference::Inline(
                        BinarySource {
                            data: "Hello world!".as_bytes().to_vec(),
                            binary_type: BinaryType {
                                mime_type: "application/json".to_string(),
                            },
                        },
                    )),
                },
                NamedElementValue {
                    name: "x".to_string(),
                    value: ElementValue::ComponentModel(101u32.into_value_and_type()),
                },
            ],
        }),
    )
}

#[test]
fn invalid_agent_type() {
    failure_test(
        "unknown-agent",
        DataValue::Tuple(ElementValues { elements: vec![] }),
        "Unknown agent type: unknown-agent",
    )
}

#[test]
fn invalid_agent_param_count() {
    failure_test(
        "agent-1",
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(12u32.into_value_and_type())],
        }),
        "Unexpected number of parameters: got 1, expected 0",
    )
}

#[test]
fn invalid_agent_param_type() {
    failure_test(
        "agent-2",
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel("hello".into_value_and_type())],
        }),
        "Failed to parse parameter value \"hello\": invalid value type at 0..7",
    )
}

#[test]
fn invalid_text_url() {
    failure_test(
        "agent-4",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::UnstructuredText(TextReference::Url(Url {
                    value: "https://url1.com/".to_string(),
                })),
                ElementValue::UnstructuredText(TextReference::Url(Url {
                    value: "not?a/valid!url".to_string(),
                })),
            ],
        }),
        "Failed to parse parameter value not?a/valid!url as URL: relative URL without a base",
    )
}

#[test]
fn roundtrip_test_with_phantom_id() {
    let phantom_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
    roundtrip_test_with_id(
        "agent-1",
        DataValue::Tuple(ElementValues { elements: vec![] }),
        Some(phantom_id),
    )
}

#[test]
fn roundtrip_test_phantom_id_complex() {
    let phantom_id = Uuid::parse_str("f47ac10b-58cc-4372-a567-0e02b2c3d479").unwrap();
    roundtrip_test_with_id(
        "agent-3",
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(12u32.into_value_and_type()),
                ElementValue::ComponentModel(ValueAndType::new(
                    Value::Record(vec![
                        Value::U32(1),
                        Value::U32(2),
                        Value::Flags(vec![true, false, true]),
                    ]),
                    record(vec![
                        field("x", u32()),
                        field("y", u32()),
                        field("properties", flags(&["a", "b", "c"])),
                    ]),
                )),
            ],
        }),
        Some(phantom_id),
    )
}

#[test]
fn invalid_phantom_id() {
    failure_test_with_string(
        "agent-1()[not-a-uuid]",
        "Invalid UUID in phantom ID: invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `n` at 1",
    )
}

#[test]
fn roundtrip_without_phantom_id_maintains_none() {
    roundtrip_test_with_id(
        "agent-1",
        DataValue::Tuple(ElementValues { elements: vec![] }),
        None,
    )
}

#[test]
fn roundtrip_with_non_kebab_metadata() {
    roundtrip_test(
        "non-kebab-agent",
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ValueAndType::new(
                // promiseId
                Value::Record(vec![
                    // agentId
                    Value::Record(vec![
                        // componentId
                        Value::Record(vec![
                            // uuid
                            Value::Record(vec![
                                Value::U64(115746831381919841),   // highBits
                                Value::U64(13556493125794766855), // lowBits
                            ]),
                        ]),
                        // agentId
                        Value::String("some-agent-id(\"hello\")".to_string()),
                    ]),
                    Value::U64(1234), // oplogIdx
                ]),
                record(vec![
                    field(
                        "agentId",
                        record(vec![
                            field(
                                "componentId",
                                record(vec![field(
                                    "uuid",
                                    record(vec![field("highBits", u64()), field("lowBits", u64())]),
                                )]),
                            ),
                            field("agentId", str()),
                        ]),
                    ),
                    field("oplogIdx", u64()),
                ]),
            ))],
        }),
    );
}

#[test]
fn untyped_data_value_serde_poem_roundtrip() {
    let original = UntypedJsonDataValue::Tuple(UntypedJsonElementValues {
        elements: vec![
            UntypedJsonElementValue::ComponentModel(JsonComponentModelValue {
                value: 42u32.into_value_and_type().to_json_value().unwrap(),
            }),
            UntypedJsonElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Url(Url {
                    value: "https://example.com/".to_string(),
                }),
            }),
        ],
    });

    let poem_serialized = original.to_json_string();
    println!("{}", poem_serialized);
    let serde_deserialized: UntypedJsonDataValue = serde_json::from_str(&poem_serialized).unwrap();
    assert_eq!(original, serde_deserialized);
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
        AgentId::normalize_text("agent-7(  [  12,     13 , 14 ]   )").unwrap(),
        "agent-7([12,13,14])"
    );
}

#[test]
fn normalize_strips_whitespace_in_records() {
    assert_eq!(
        AgentId::normalize_text(
            r#"agent-3(  32 ,{ x  : 12, y: 32, properties: {a,    b  , c   } })"#
        )
        .unwrap(),
        "agent-3(32,{x:12,y:32,properties:{a,b,c}})"
    );
}

#[test]
fn normalize_preserves_already_compact() {
    assert_eq!(AgentId::normalize_text("agent-1()").unwrap(), "agent-1()");
}

#[test]
fn normalize_preserves_strings() {
    assert_eq!(
        AgentId::normalize_text(r#"agent-2("hello world")"#).unwrap(),
        r#"agent-2("hello world")"#
    );
}

#[test]
fn normalize_handles_phantom_id() {
    let result =
        AgentId::normalize_text("agent-1()[550e8400-e29b-41d4-a716-446655440000]").unwrap();
    assert_eq!(result, "agent-1()[550e8400-e29b-41d4-a716-446655440000]");
}

#[test]
fn normalize_handles_phantom_id_with_whitespace() {
    let result =
        AgentId::normalize_text("agent-1()[ 550e8400-e29b-41d4-a716-446655440000 ]").unwrap();
    assert_eq!(result, "agent-1()[550e8400-e29b-41d4-a716-446655440000]");
}

#[test]
fn normalize_rejects_invalid_format() {
    assert!(AgentId::normalize_text("not-an-agent-id").is_err());
}

#[test]
fn normalize_rejects_invalid_phantom_id() {
    assert!(AgentId::normalize_text("agent-1()[not-a-uuid]").is_err());
}

#[test]
fn normalize_handles_urls() {
    assert_eq!(
        AgentId::normalize_text("agent-4(https://url1.com/,https://url2.com/)").unwrap(),
        "agent-4(https://url1.com/,https://url2.com/)"
    );
}

#[test]
fn normalize_handles_inline_text() {
    assert_eq!(
        AgentId::normalize_text(r#"agent-4("hello, world!","goodbye")"#).unwrap(),
        r#"agent-4("hello, world!","goodbye")"#
    );
}

#[test]
fn normalize_handles_multimodal_elements() {
    assert_eq!(
        AgentId::normalize_text("agent-6(x(  42  ),y(https://example.com/))").unwrap(),
        "agent-6(x(42),y(https://example.com/))"
    );
}

#[test]
fn normalize_handles_nested_records_with_whitespace() {
    assert_eq!(
        AgentId::normalize_text(
            r#"non-kebab-agent({ agent-id : { component-id : { uuid : { high-bits : 115746831381919841 , low-bits : 13556493125794766855 } } , agent-id : "some-agent-id(\"hello\")" } , oplog-idx : 1234 })"#
        )
        .unwrap(),
        r#"non-kebab-agent({agent-id:{component-id:{uuid:{high-bits:115746831381919841,low-bits:13556493125794766855}},agent-id:"some-agent-id(\"hello\")"},oplog-idx:1234})"#
    );
}

#[test]
fn normalize_handles_options_and_results() {
    assert_eq!(
        AgentId::normalize_text("agent-x( some( 42 ) )").unwrap(),
        "agent-x(some(42))"
    );
    assert_eq!(
        AgentId::normalize_text("agent-x( none )").unwrap(),
        "agent-x(none)"
    );
    assert_eq!(
        AgentId::normalize_text("agent-x( ok( 1 ) )").unwrap(),
        "agent-x(ok(1))"
    );
    assert_eq!(
        AgentId::normalize_text("agent-x( err( 2 ) )").unwrap(),
        "agent-x(err(2))"
    );
}

#[test]
fn normalize_handles_empty_record() {
    assert_eq!(
        AgentId::normalize_text("agent-x( {  :  } )").unwrap(),
        "agent-x({:})"
    );
}

#[test]
fn normalize_handles_empty_flags() {
    assert_eq!(
        AgentId::normalize_text("agent-x( {  } )").unwrap(),
        "agent-x({})"
    );
}

#[test]
fn normalize_handles_char_values() {
    assert_eq!(
        AgentId::normalize_text("agent-x( 'a' , 'b' )").unwrap(),
        "agent-x('a','b')"
    );
}

#[test]
fn normalize_handles_variant_with_percent_prefix() {
    assert_eq!(
        AgentId::normalize_text("agent-x( %true( 42 ) )").unwrap(),
        "agent-x(%true(42))"
    );
}

#[test]
fn normalize_trims_outer_whitespace() {
    assert_eq!(
        AgentId::normalize_text("  agent-7(  [  12, 13 ]  )  ").unwrap(),
        "agent-7([12,13])"
    );
}

#[test]
fn normalize_phantom_id_with_casing_and_whitespace() {
    let result =
        AgentId::normalize_text("agent-1(  )[ 550E8400-E29B-41D4-A716-446655440000 ]").unwrap();
    assert_eq!(result, "agent-1()[550e8400-e29b-41d4-a716-446655440000]");
}

#[test]
fn normalize_empty_params_stays_empty() {
    assert_eq!(
        AgentId::normalize_text("agent-1(   )").unwrap(),
        "agent-1()"
    );
}

#[test]
fn normalize_rejects_empty_element_from_double_comma() {
    assert!(AgentId::normalize_text("agent-x(1,,2)").is_err());
}

#[test]
fn normalize_rejects_leading_comma() {
    assert!(AgentId::normalize_text("agent-x(,1)").is_err());
}

#[test]
fn normalize_rejects_empty_agent_type() {
    assert!(AgentId::normalize_text("()").is_err());
}

proptest! {
    #[test]
    fn normalize_text_idempotent_for_simple_agent(x in 0u32..10000) {
        let agent_id = AgentId::new(
            AgentTypeName("agent-2".to_string()),
            DataValue::Tuple(ElementValues {
                elements: vec![
                    ElementValue::ComponentModel(x.into_value_and_type()),
                ],
            }),
            None,
        );
        let canonical = agent_id.to_string();
        let normalized = AgentId::normalize_text(&canonical).unwrap();
        prop_assert_eq!(&normalized, &canonical);
    }

    #[test]
    fn normalize_text_idempotent_for_list_agent(
        a in 0u32..100,
        b in 0u32..100,
        c in 0u32..100,
    ) {
        let agent_id = AgentId::new(
            AgentTypeName("agent-7".to_string()),
            DataValue::Tuple(ElementValues {
                elements: vec![
                    ElementValue::ComponentModel(ValueAndType::new(
                        Value::List(vec![Value::U32(a), Value::U32(b), Value::U32(c)]),
                        list(u32()),
                    )),
                ],
            }),
            None,
        );
        let canonical = agent_id.to_string();
        let normalized = AgentId::normalize_text(&canonical).unwrap();
        prop_assert_eq!(&normalized, &canonical);
    }

    #[test]
    fn normalize_text_strips_whitespace_for_simple_agent(x in 0u32..10000) {
        let with_spaces = format!("agent-2(  {x}  )");
        let normalized = AgentId::normalize_text(&with_spaces).unwrap();
        prop_assert_eq!(normalized, format!("agent-2({x})"));
    }

    #[test]
    fn normalize_text_strips_whitespace_for_list(
        a in 0u32..100,
        b in 0u32..100,
        c in 0u32..100,
    ) {
        let with_spaces = format!("agent-7( [ {a} , {b} , {c} ] )");
        let normalized = AgentId::normalize_text(&with_spaces).unwrap();
        prop_assert_eq!(normalized, format!("agent-7([{a},{b},{c}])"));
    }
}

fn roundtrip_test(agent_type: &str, parameters: DataValue) {
    let id = AgentId::new(AgentTypeName(agent_type.to_string()), parameters, None);
    let s = id.to_string();
    println!("{s}");
    let id2 = AgentId::parse(s, TestAgentTypes::new()).unwrap();
    assert_eq!(id, id2);
}

fn roundtrip_test_with_id(agent_type: &str, parameters: DataValue, phantom_id: Option<Uuid>) {
    let id = AgentId::new(
        AgentTypeName(agent_type.to_string()),
        parameters,
        phantom_id,
    );
    let s = id.to_string();
    println!("{s}");
    let id2 = AgentId::parse(s, TestAgentTypes::new()).unwrap();
    assert_eq!(id, id2);
    assert_eq!(id.phantom_id, phantom_id);
}

fn failure_test(agent_type: &str, parameters: DataValue, expected_failure: &str) {
    let id = AgentId::new(AgentTypeName(agent_type.to_string()), parameters, None);
    let s = id.to_string();
    let id2 = AgentId::parse(s, TestAgentTypes::new()).err().unwrap();
    assert_eq!(id2, expected_failure.to_string());
}

fn failure_test_with_string(agent_id_str: &str, expected_failure: &str) {
    let id2 = AgentId::parse(agent_id_str, TestAgentTypes::new())
        .err()
        .unwrap();
    assert_eq!(id2, expected_failure.to_string());
}

struct TestAgentTypes {
    types: HashMap<AgentTypeName, AgentType>,
}

impl TestAgentTypes {
    pub fn new() -> Self {
        Self {
            types: test_agent_types(),
        }
    }
}

#[async_trait]
impl AgentTypeResolver for TestAgentTypes {
    fn resolve_agent_type_by_wrapper_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentType, String> {
        self.types
            .get(agent_type)
            .cloned()
            .ok_or_else(|| format!("Unknown agent type: {}", agent_type))
    }
}

fn test_agent_types() -> HashMap<AgentTypeName, AgentType> {
    let agent_types = &[
        AgentType {
            type_name: AgentTypeName("agent-1".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("agent-2".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "x".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: u32(),
                        }),
                    }],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("agent-3".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "x".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: u32(),
                            }),
                        },
                        NamedElementSchema {
                            name: "r".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: record(vec![
                                    field("x", u32()),
                                    field("y", u32()),
                                    field("properties", flags(&["a", "b", "c"])),
                                ]),
                            }),
                        },
                    ],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("agent-4".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "a".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        },
                        NamedElementSchema {
                            name: "b".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        },
                    ],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("agent-5".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "a".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: None,
                            }),
                        },
                        NamedElementSchema {
                            name: "b".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: None,
                            }),
                        },
                    ],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("agent-6".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Multimodal(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "x".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: u32(),
                            }),
                        },
                        NamedElementSchema {
                            name: "y".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        },
                        NamedElementSchema {
                            name: "z".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: None,
                            }),
                        },
                    ],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("agent-7".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "args".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: list(u32()),
                        }),
                    }],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
        AgentType {
            type_name: AgentTypeName("non-kebab-agent".to_string()),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "args".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![
                                field(
                                    "agentId",
                                    record(vec![
                                        field(
                                            "componentId",
                                            record(vec![field(
                                                "uuid",
                                                record(vec![
                                                    field("highBits", u64()),
                                                    field("lowBits", u64()),
                                                ]),
                                            )]),
                                        ),
                                        field("agentId", str()),
                                    ]),
                                ),
                                field("oplogIdx", u64()),
                            ]),
                        }),
                    }],
                }),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
        },
    ];

    let mut result = HashMap::new();
    for agent_type in agent_types {
        result.insert(agent_type.type_name.clone(), agent_type.clone());
    }
    result
}

#[test]
fn data_value_macro_empty() {
    let value = data_value!();
    assert_eq!(value, DataValue::Tuple(ElementValues { elements: vec![] }));
}

#[test]
fn data_value_macro_single_u32() {
    let value = data_value!(42u32);
    assert_eq!(
        value,
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(42u32.into_value_and_type())]
        })
    );
}

#[test]
fn data_value_macro_multiple_primitives() {
    let value = data_value!(42u32, 100u64, 3u8);
    assert_eq!(
        value,
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(42u32.into_value_and_type()),
                ElementValue::ComponentModel(100u64.into_value_and_type()),
                ElementValue::ComponentModel(3u8.into_value_and_type()),
            ]
        })
    );
}

#[test]
fn data_value_macro_mixed_types() {
    let value = data_value!(42u32, 3u8);
    let elements = match value {
        DataValue::Tuple(ElementValues { elements }) => elements,
        _ => panic!("Expected DataValue::Tuple"),
    };

    assert_eq!(elements.len(), 2);

    // Verify first element is a ComponentModel
    match &elements[0] {
        ElementValue::ComponentModel(vat) => {
            assert_eq!(vat.value, Value::U32(42));
        }
        _ => panic!("Expected ComponentModel"),
    }

    // Verify second element is a ComponentModel
    match &elements[1] {
        ElementValue::ComponentModel(vat) => {
            assert_eq!(vat.value, Value::U8(3));
        }
        _ => panic!("Expected ComponentModel"),
    }
}

#[test]
fn data_value_macro_trailing_comma() {
    let value = data_value!(42u32, 100u64,);
    assert_eq!(
        value,
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(42u32.into_value_and_type()),
                ElementValue::ComponentModel(100u64.into_value_and_type()),
            ]
        })
    );
}

#[test]
fn agent_id_macro_no_parameters() {
    let id = agent_id!("agent-1");
    assert_eq!(id.agent_type, AgentTypeName("agent-1".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues { elements: vec![] })
    );
    assert_eq!(id.phantom_id, None);
}

#[test]
fn agent_id_macro_single_parameter() {
    let id = agent_id!("agent-2", 42u32);
    assert_eq!(id.agent_type, AgentTypeName("agent-2".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(42u32.into_value_and_type())]
        })
    );
    assert_eq!(id.phantom_id, None);
}

#[test]
fn agent_id_macro_multiple_parameters() {
    let id = agent_id!("agent-3", 42u32, 100u64, 3u8);
    assert_eq!(id.agent_type, AgentTypeName("agent-3".to_string()));
    let expected_params = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(42u32.into_value_and_type()),
            ElementValue::ComponentModel(100u64.into_value_and_type()),
            ElementValue::ComponentModel(3u8.into_value_and_type()),
        ],
    });
    assert_eq!(id.parameters, expected_params);
    assert_eq!(id.phantom_id, None);
}

#[test]
fn agent_id_macro_with_trailing_comma() {
    let id = agent_id!("agent-4", 42u32, 100u64,);
    assert_eq!(id.agent_type, AgentTypeName("agent-4".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(42u32.into_value_and_type()),
                ElementValue::ComponentModel(100u64.into_value_and_type()),
            ]
        })
    );
    assert_eq!(id.phantom_id, None);
}

#[test]
fn phantom_agent_id_macro_no_parameters() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-1", phantom_uuid);
    assert_eq!(id.agent_type, AgentTypeName("phantom-1".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues { elements: vec![] })
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn phantom_agent_id_macro_single_parameter() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-2", phantom_uuid, 42u32);
    assert_eq!(id.agent_type, AgentTypeName("phantom-2".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(42u32.into_value_and_type())]
        })
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn phantom_agent_id_macro_multiple_parameters() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-3", phantom_uuid, 42u32, 100u64);
    assert_eq!(id.agent_type, AgentTypeName("phantom-3".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(42u32.into_value_and_type()),
                ElementValue::ComponentModel(100u64.into_value_and_type()),
            ]
        })
    );
    assert_eq!(id.phantom_id, Some(phantom_uuid));
}

#[test]
fn phantom_agent_id_macro_with_trailing_comma() {
    let phantom_uuid = Uuid::now_v7();
    let id = phantom_agent_id!("phantom-4", phantom_uuid, 42u32, 100u64,);
    assert_eq!(id.agent_type, AgentTypeName("phantom-4".to_string()));
    assert_eq!(
        id.parameters,
        DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(42u32.into_value_and_type()),
                ElementValue::ComponentModel(100u64.into_value_and_type()),
            ]
        })
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
