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

use crate::model::agent::wit_naming::ToWitNaming;
use crate::model::agent::{
    AgentConstructor, AgentId, AgentType, AgentTypeResolver, BinaryDescriptor, BinaryReference,
    BinarySource, BinaryType, ComponentModelElementSchema, DataSchema, DataValue, ElementSchema,
    ElementValue, ElementValues, NamedElementSchema, NamedElementSchemas, NamedElementValue,
    NamedElementValues, TextDescriptor, TextReference, TextSource, TextType, Url,
};
use async_trait::async_trait;
use golem_wasm_ast::analysis::analysed_type::{field, flags, list, record, u32};
use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use test_r::test;

#[test]
fn agent_id_wave_normalization() {
    {
        let agent_id = AgentId::parse("agent-7(  [  12,     13 , 14 ]   )", TestAgentTypes::new()).unwrap();
        assert_eq!(agent_id.to_string(), "agent-7([12,13,14])");
    }

    {
        let agent_id = AgentId::parse(
            r#"agent-3(  32 ,{ x  : 12, y: 32, properties: {a,    b  , c   } })"#,
            TestAgentTypes::new(),
        )
        .unwrap();
        assert_eq!(agent_id.to_string(), "agent-3(32,{x:12,y:32,properties:{a,b,c}})");
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

fn roundtrip_test(agent_type: &str, parameters: DataValue) {
    let id = AgentId {
        agent_type: agent_type.to_string(),
        parameters,
    };
    let s = id.to_string();
    println!("{s}");
    let id2 = AgentId::parse(s, TestAgentTypes::new()).unwrap();
    assert_eq!(id, id2);
}

fn failure_test(agent_type: &str, parameters: DataValue, expected_failure: &str) {
    let id = AgentId {
        agent_type: agent_type.to_string(),
        parameters,
    };
    let s = id.to_string();
    let id2 = AgentId::parse(s, TestAgentTypes::new()).err().unwrap();
    assert_eq!(id2, expected_failure.to_string());
}

struct TestAgentTypes {
    types: HashMap<String, AgentType>,
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
    fn resolve_wit_agent_type(&self, agent_type: &str) -> Result<AgentType, String> {
        self.types
            .get(agent_type)
            .map(|agent_type| agent_type.to_wit_naming())
            .ok_or_else(|| format!("Unknown agent type: {}", agent_type))
    }
}

fn test_agent_types() -> HashMap<String, AgentType> {
    let agent_types = &[
        AgentType {
            type_name: "agent-1".to_string(),
            description: "".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
            },
            methods: vec![],
            dependencies: vec![],
        },
        AgentType {
            type_name: "agent-2".to_string(),
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
        },
        AgentType {
            type_name: "agent-3".to_string(),
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
        },
        AgentType {
            type_name: "agent-4".to_string(),
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
        },
        AgentType {
            type_name: "agent-5".to_string(),
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
        },
        AgentType {
            type_name: "agent-6".to_string(),
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
        },
        AgentType {
            type_name: "agent-7".to_string(),
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
        },
    ];

    let mut result = HashMap::new();
    for agent_type in agent_types {
        result.insert(agent_type.type_name.clone(), agent_type.clone());
    }
    result
}
