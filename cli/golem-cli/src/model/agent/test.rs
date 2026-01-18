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

use golem_common::model::agent::{
    AgentConstructor, AgentMethod, AgentMode, AgentType, BinaryDescriptor, BinaryType,
    ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
    NamedElementSchemas, TextDescriptor,
};
use golem_wasm::analysis::analysed_type::{
    bool, case, chr, field, list, option, r#enum, record, result, s32, str, tuple, u32, u8,
    unit_case, unit_result, variant,
};

pub fn single_agent_wrapper_types() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("agent1".to_string()),
        description: "An example agent".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "Creates an example agent instance".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "a".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: u32(),
                        }),
                    },
                    NamedElementSchema {
                        name: "b".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: option(str()),
                        }),
                    },
                ],
            }),
        },
        methods: vec![
            AgentMethod {
                name: "f1".to_string(),
                description: "returns a random string".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                output_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "a".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
                http_endpoint: Vec::new(),
            },
            AgentMethod {
                name: "f2".to_string(),
                description: "adds two numbers".to_string(),
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
                            name: "y".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: u32(),
                            }),
                        },
                    ],
                }),
                output_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "return".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: u32(),
                        }),
                    }],
                }),
                http_endpoint: Vec::new(),
            },
        ],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn multi_agent_wrapper_2_types() -> Vec<AgentType> {
    let color = r#enum(&["red", "green", "blue"]).named("color");

    let person = record(vec![
        field("first-name", str()),
        field("last-name", str()),
        field("age", option(u32())),
        field("eye-color", color.clone()),
    ])
    .named("person");

    let location = variant(vec![
        case("home", str()),
        case("work", str()),
        unit_case("unknown"),
    ])
    .named("location");

    let agent_types = vec![
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("agent1".to_string()),
            description: "An example agent".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "Creates an example agent instance".into(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "person".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: person.clone(),
                            }),
                        },
                        NamedElementSchema {
                            name: "description".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        },
                        NamedElementSchema {
                            name: "photo".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: None,
                            }),
                        },
                    ],
                }),
            },
            methods: vec![AgentMethod {
                name: "f1".to_string(),
                description: "returns a location".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                output_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "return".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: location.clone(),
                        }),
                    }],
                }),
                http_endpoint: Vec::new(),
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("agent2".to_string()),
            description: "Another example agent".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "Creates another example agent instance".into(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "person-group".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: list(person),
                        }),
                    }],
                }),
            },
            methods: vec![AgentMethod {
                name: "f2".to_string(),
                description: "takes a location or a color and returns a text or an image"
                    .to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Multimodal(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "place".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: location,
                            }),
                        },
                        NamedElementSchema {
                            name: "color".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: color,
                            }),
                        },
                    ],
                }),
                output_schema: DataSchema::Multimodal(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        },
                        NamedElementSchema {
                            name: "image".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: None,
                            }),
                        },
                    ],
                }),
                http_endpoint: Vec::new(),
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
    ];

    agent_types
}

pub fn agent_type_with_wit_keywords() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("agent1".to_string()),
        description: "An example agent using WIT keywords as names".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "Creates an example agent instance".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "export".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: u32(),
                        }),
                    },
                    NamedElementSchema {
                        name: "func".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: option(str()),
                        }),
                    },
                ],
            }),
        },
        methods: vec![
            AgentMethod {
                name: "import".to_string(),
                description: "returns a random string".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                output_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "interface".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
                http_endpoint: Vec::new(),
            },
            AgentMethod {
                name: "package".to_string(),
                description: "adds two numbers".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: crate::model::agent::wit::WIT_KEYWORDS
                        .iter()
                        .map(|keyword| NamedElementSchema {
                            name: keyword.to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: u32(),
                            }),
                        })
                        .collect(),
                }),
                output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                http_endpoint: Vec::new(),
            },
        ],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn reproducer_for_multiple_types_called_element() -> Vec<AgentType> {
    vec![
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("assistant-agent".to_string()),
            description: "AssistantAgent".to_string(),
            constructor: AgentConstructor {
                name: Some("AssistantAgent".to_string()),
                description: "Constructs [object Object]".to_string(),
                prompt_hint: Some("Enter something...".to_string()),
                input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            },
            methods: vec![AgentMethod {
                name: "ask_more".to_string(),
                description: "".to_string(),
                prompt_hint: Some("".to_string()),
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "name".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
                output_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "return-value".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![field("x", str())]),
                        }),
                    }],
                }),
                http_endpoint: Vec::new(),
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("weather-agent".to_string()),
            description: "WeatherAgent".to_string(),
            constructor: AgentConstructor {
                name: Some("WeatherAgent".to_string()),
                description: "Constructs [object Object]".to_string(),
                prompt_hint: Some("Enter something...".to_string()),
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "username".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
            },
            methods: vec![AgentMethod {
                name: "getWeather".to_string(),
                description: "Weather forecast weather for you".to_string(),
                prompt_hint: Some("".to_string()),
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "name".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        },
                        NamedElementSchema {
                            name: "param2".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: record(vec![
                                    field("data", str()),
                                    field("value", s32()),
                                ]),
                            }),
                        },
                    ],
                }),
                output_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "return-value".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
                http_endpoint: Vec::new(),
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
    ]
}

pub fn reproducer_for_issue_with_enums() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("foo-agent".to_string()),
        description: "FooAgent".to_string(),
        constructor: AgentConstructor {
            name: Some("FooAgent".to_string()),
            description: "".to_string(),
            prompt_hint: Some("Enter something...".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "input".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: str(),
                    }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "myFun".to_string(),
            description: "".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "param".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: r#enum(&["foo", "bar", "baz"])
                            .named("union-with-only-literals"),
                    }),
                }],
            }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "return-value".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: r#enum(&["foo", "bar", "baz"])
                            .named("union-with-only-literals"),
                    }),
                }],
            }),
            http_endpoint: Vec::new(),
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn reproducer_for_issue_with_result_types() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("bar-agent".to_string()),
        description: "Constructs the agent bar-agent".to_string(),
        constructor: AgentConstructor {
            name: Some("BarAgent".to_string()),
            description: "Constructs the agent bar-agent".to_string(),
            prompt_hint: Some("Enter the following parameters: ".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        },
        methods: vec![AgentMethod {
            name: "funEither".to_string(),
            description: "".to_string(),
            prompt_hint: Some("".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "either".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: result(str(), str()).named("result-exact"),
                    }),
                }],
            }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "return-value".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: result(str(), str()).named("result-exact"),
                    }),
                }],
            }),
            http_endpoint: Vec::new(),
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn multimodal_untagged_variant_in_out() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("test-agent".to_string()),
        description: "Test".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "Constructor".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        },
        methods: vec![AgentMethod {
            name: "foo".to_string(),
            description: "".to_string(),
            prompt_hint: Some("".to_string()),
            input_schema: DataSchema::Multimodal(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "text".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![field("val", str())]).named("text"),
                        }),
                    },
                    NamedElementSchema {
                        name: "image".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![field("val", list(u8()))]).named("image"),
                        }),
                    },
                ],
            }),
            output_schema: DataSchema::Multimodal(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "text".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![field("val", str())]).named("text"),
                        }),
                    },
                    NamedElementSchema {
                        name: "image".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![field("val", list(u8()))]).named("image"),
                        }),
                    },
                ],
            }),
            http_endpoint: Vec::new(),
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn char_type() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("agent-using-char".to_string()),
        description: "An example agent".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "Creates an example agent instance".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "a".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: chr(),
                    }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "f1".to_string(),
            description: "returns a random string".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "a".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: chr(),
                    }),
                }],
            }),
            http_endpoint: Vec::new(),
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn unit_result_type() -> Vec<AgentType> {
    vec![AgentType {
        type_name: golem_common::model::agent::AgentTypeName("agent-unit-result".to_string()),
        description: "An example agent".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "Creates an example agent instance".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "a".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: unit_result(),
                    }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "f1".to_string(),
            description: "returns a random string".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "a".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: unit_result(),
                    }),
                }],
            }),
            http_endpoint: Vec::new(),
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
    }]
}

pub fn ts_code_first_snippets() -> Vec<AgentType> {
    // Define reusable types with names
    let object_type = record(vec![
        field("a", str()),
        field("b", s32()),
        field("c", bool()),
    ])
    .named("ObjectType");

    let union_type = variant(vec![
        case("case1", s32()),
        case("case2", str()),
        case("case3", bool()),
        case("case4", object_type.clone()),
    ])
    .named("UnionType");

    let map_type = list(tuple(vec![str(), s32()])).named("MapType");

    let tuple_complex_type =
        tuple(vec![str(), s32(), object_type.clone()]).named("TupleComplexType");

    let tuple_type = tuple(vec![str(), s32(), bool()]).named("TupleType");

    let simple_interface_type = record(vec![field("n", s32())]).named("SimpleInterfaceType");

    let object_complex_type = record(vec![
        field("a", str()),
        field("b", s32()),
        field("c", bool()),
        field("d", object_type.clone()),
        field("e", union_type.clone()),
        field("f", list(str())),
        field("g", list(object_type.clone())),
        field("h", tuple_type.clone()),
        field("i", tuple_complex_type.clone()),
        field("j", map_type.clone()),
        field("k", simple_interface_type.clone()),
    ])
    .named("ObjectComplexType");

    let list_complex_type = list(object_type.clone()).named("ListComplexType");

    let union_complex_type = variant(vec![
        case("case1", s32()),
        case("case2", str()),
        case("case3", bool()),
        case("case4", object_complex_type.clone()),
        case("case5", union_type.clone()),
        case("case6", tuple_type.clone()),
        case("case7", tuple_complex_type.clone()),
        case("case8", simple_interface_type.clone()),
        case("case9", list(str())),
    ])
    .named("UnionComplexType");

    let result_like_with_no_tag = record(vec![
        field("ok", option(str())),
        field("err", option(str())),
    ])
    .named("ResultLikeWithNoTag");

    let result_like =
        variant(vec![case("okay", str()), case("error", option(str()))]).named("ResultLike");

    let result_exact = variant(vec![case("ok", str()), case("err", str())]).named("ResultExact");

    let result_like_with_void =
        variant(vec![unit_case("ok"), unit_case("err")]).named("ResultLikeWithVoid");

    let object_with_union_undefined_1 =
        record(vec![field("a", option(str()))]).named("ObjectWithUnionWithUndefined1");

    let object_with_union_undefined_2 =
        record(vec![field("a", option(str()))]).named("ObjectWithUnionWithUndefined2");

    let object_with_union_undefined_3 =
        record(vec![field("a", option(str()))]).named("ObjectWithUnionWithUndefined3");

    let object_with_union_undefined_4 =
        record(vec![field("a", option(str()))]).named("ObjectWithUnionWithUndefined4");

    let tagged_union = variant(vec![
        case("a", str()),
        case("b", s32()),
        case("c", bool()),
        case("d", union_type.clone()),
        case("e", object_type.clone()),
        case("f", list(str())),
        case("g", tuple(vec![str(), s32(), bool()])),
        case("h", simple_interface_type.clone()),
        unit_case("i"),
        unit_case("j"),
    ])
    .named("TaggedUnion");

    let union_with_literals = variant(vec![
        unit_case("lit1"),
        unit_case("lit2"),
        unit_case("lit3"),
        case("union-with-literals1", bool()),
    ])
    .named("UnionWithLiterals");

    let union_with_only_literals = r#enum(&["foo", "bar", "baz"]).named("UnionWithOnlyLiterals");

    let anonymous_union_with_only_literals = r#enum(&["foo", "bar", "baz"]);

    let anonymous_union_with_only_literals2 = r#enum(&["foo", "bar", "baz", "baz2"]);

    vec![
        // FooAgent
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("FooAgent".to_string()),
            description: "FooAgent class".to_string(),
            constructor: AgentConstructor {
                name: Some("FooAgent".to_string()),
                description: "Constructor for FooAgent".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "input".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
            },
            methods: vec![
                AgentMethod {
                    name: "funAll".to_string(),
                    description: "Takes all complex types".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "complexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "unionType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: union_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "unionComplexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: union_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "numberType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: s32(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "stringType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "booleanType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: bool(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "mapType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: map_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "tupleComplexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: tuple_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "tupleType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: tuple_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "listComplexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: list_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "resultLike".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: result_like.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "resultLikeWithNoTag".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: result_like_with_no_tag.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "unionWithNull".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined1".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_1.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined2".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_2.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined3".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_3.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined4".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_4.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "optionalStringType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "optionalUnionType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(union_type.clone()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "taggedUnionType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: tagged_union.clone(),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funOptional".to_string(),
                    description: "Takes optional parameters".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "param1".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param2".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_1.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param3".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_2.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param4".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_3.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param5".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_4.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param6".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param7".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(union_type.clone()),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funOptionalQMark".to_string(),
                    description: "Takes optional parameters with question mark".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "param1".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param2".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(s32()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param3".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funObjectComplexType".to_string(),
                    description: "Takes ObjectComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionType".to_string(),
                    description: "Takes UnionType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionComplexType".to_string(),
                    description: "Takes UnionComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionComplexType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funNumber".to_string(),
                    description: "Takes NumberType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "numberType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: s32(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: s32(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funString".to_string(),
                    description: "Takes StringType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "stringType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBoolean".to_string(),
                    description: "Takes BooleanType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "booleanType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: bool(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: bool(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funMap".to_string(),
                    description: "Takes MapType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "mapType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: map_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: map_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funTaggedUnion".to_string(),
                    description: "Takes TaggedUnion".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "taggedUnionType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tagged_union.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tagged_union.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funTupleComplexType".to_string(),
                    description: "Takes TupleComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "complexType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funTupleType".to_string(),
                    description: "Takes TupleType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "tupleType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funListComplexType".to_string(),
                    description: "Takes ListComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "listComplexType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: list_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: list_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funObjectType".to_string(),
                    description: "Takes ObjectType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "objectType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionWithLiterals".to_string(),
                    description: "Takes UnionWithLiterals".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionWithLiterals".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_literals.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_literals.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionWithOnlyLiterals".to_string(),
                    description: "Takes UnionWithOnlyLiterals".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionWithLiterals".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_only_literals.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_only_literals.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funAnonymousUnionWithOnlyLiterals".to_string(),
                    description: "Takes AnonymousUnionWithOnlyLiterals".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "anonymousUnionWithLiterals".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: anonymous_union_with_only_literals.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: anonymous_union_with_only_literals.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funAnonymousUnionWithOnlyLiterals2".to_string(),
                    description: "Takes AnonymousUnionWithOnlyLiterals".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "anonymousUnionWithLiterals".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: anonymous_union_with_only_literals2.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: anonymous_union_with_only_literals2.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funVoidReturn".to_string(),
                    description: "Returns void".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funNullReturn".to_string(),
                    description: "Returns null".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUndefinedReturn".to_string(),
                    description: "Returns undefined".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnstructuredText".to_string(),
                    description: "Takes UnstructuredText".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unstructuredText".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnstructuredBinary".to_string(),
                    description: "Takes UnstructuredBinary".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unstructuredText".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: Some(vec![BinaryType {
                                    mime_type: "application/json".to_string(),
                                }]),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funMultimodal".to_string(),
                    description: "Takes Multimodal".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::UnstructuredText(TextDescriptor {
                                    restrictions: None,
                                }),
                            },
                            NamedElementSchema {
                                name: "binary".to_string(),
                                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                    restrictions: None,
                                }),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::UnstructuredText(TextDescriptor {
                                    restrictions: None,
                                }),
                            },
                            NamedElementSchema {
                                name: "binary".to_string(),
                                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                    restrictions: None,
                                }),
                            },
                        ],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funMultimodalAdvanced".to_string(),
                    description: "Takes MultimodalAdvanced".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "image".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: list(u8()),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "image".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: list(u8()),
                                    },
                                ),
                            },
                        ],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funEitherOptional".to_string(),
                    description: "Takes ResultLikeWithNoTag".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "eitherBothOptional".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_no_tag.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_no_tag.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultExact".to_string(),
                    description: "Takes ResultExact".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "either".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_exact.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_exact.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultLike".to_string(),
                    description: "Takes ResultLike".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "eitherOneOptional".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultLikeWithVoid".to_string(),
                    description: "Takes ResultLikeWithVoid".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "resultLikeWithVoid".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_void.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_void.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBuiltinResultVS".to_string(),
                    description: "Takes Result<void, string>".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "result".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultVoidString"),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultVoidString"),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBuiltinResultSV".to_string(),
                    description: "Takes Result<string, void>".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "result".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringVoid"),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringVoid"),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBuiltinResultSN".to_string(),
                    description: "Takes Result<string, number>".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "result".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringNumber"),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringNumber"),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funNoReturn".to_string(),
                    description: "Returns nothing".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funArrowSync".to_string(),
                    description: "Arrow function returning text".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
            ],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
        // BarAgent
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("BarAgent".to_string()),
            description: "BarAgent class".to_string(),
            constructor: AgentConstructor {
                name: Some("BarAgent".to_string()),
                description: "Constructor for BarAgent".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "optionalStringType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: option(str()),
                            }),
                        },
                        NamedElementSchema {
                            name: "optionalUnionType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: option(union_type.clone()),
                            }),
                        },
                    ],
                }),
            },
            methods: vec![
                AgentMethod {
                    name: "funAll".to_string(),
                    description: "Takes all complex types".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "complexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "unionType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: union_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "unionComplexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: union_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "numberType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: s32(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "stringType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "booleanType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: bool(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "mapType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: map_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "tupleComplexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: tuple_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "tupleType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: tuple_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "listComplexType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: list_complex_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_type.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "resultLike".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: result_like.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "resultLikeWithNoTag".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: result_like_with_no_tag.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "unionWithNull".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined1".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_1.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined2".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_2.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined3".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_3.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "objectWithUnionWithUndefined4".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_4.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "optionalStringType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "optionalUnionType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(union_type.clone()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "taggedUnionType".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: tagged_union.clone(),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funOptional".to_string(),
                    description: "Takes optional parameters".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "param1".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param2".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_1.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param3".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_2.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param4".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_3.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param5".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: object_with_union_undefined_4.clone(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param6".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param7".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(union_type.clone()),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funOptionalQMark".to_string(),
                    description: "Takes optional parameters with question mark".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "param1".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param2".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(s32()),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "param3".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: option(str()),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funObjectComplexType".to_string(),
                    description: "Takes ObjectComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionType".to_string(),
                    description: "Takes UnionType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionComplexType".to_string(),
                    description: "Takes UnionComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionComplexType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funNumber".to_string(),
                    description: "Takes NumberType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "numberType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: s32(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: s32(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funString".to_string(),
                    description: "Takes StringType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "stringType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBoolean".to_string(),
                    description: "Takes BooleanType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "booleanType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: bool(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: bool(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funText".to_string(),
                    description: "Takes MapType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "mapType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: map_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: map_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funTupleComplexType".to_string(),
                    description: "Takes TupleComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "complexType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funTupleType".to_string(),
                    description: "Takes TupleType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "tupleType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tuple_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funListComplexType".to_string(),
                    description: "Takes ListComplexType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "listComplexType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: list_complex_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: list_complex_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funObjectType".to_string(),
                    description: "Takes ObjectType".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "objectType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_type.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: object_type.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionWithLiterals".to_string(),
                    description: "Takes UnionWithLiterals".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionWithLiterals".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_literals.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_literals.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funVoidReturn".to_string(),
                    description: "Returns void".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funNullReturn".to_string(),
                    description: "Returns null".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUndefinedReturn".to_string(),
                    description: "Returns undefined".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnstructuredText".to_string(),
                    description: "Takes UnstructuredText".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unstructuredText".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnstructuredBinary".to_string(),
                    description: "Takes UnstructuredBinary".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unstructuredText".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: Some(vec![BinaryType {
                                    mime_type: "application/json".to_string(),
                                }]),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funMultimodal".to_string(),
                    description: "Takes Multimodal".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::UnstructuredText(TextDescriptor {
                                    restrictions: None,
                                }),
                            },
                            NamedElementSchema {
                                name: "binary".to_string(),
                                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                    restrictions: None,
                                }),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::UnstructuredText(TextDescriptor {
                                    restrictions: None,
                                }),
                            },
                            NamedElementSchema {
                                name: "binary".to_string(),
                                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                    restrictions: None,
                                }),
                            },
                        ],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funMultimodalAdvanced".to_string(),
                    description: "Takes MultimodalAdvanced".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "image".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: list(u8()),
                                    },
                                ),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Multimodal(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "text".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: str(),
                                    },
                                ),
                            },
                            NamedElementSchema {
                                name: "image".to_string(),
                                schema: ElementSchema::ComponentModel(
                                    ComponentModelElementSchema {
                                        element_type: list(u8()),
                                    },
                                ),
                            },
                        ],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funUnionWithOnlyLiterals".to_string(),
                    description: "Takes UnionWithOnlyLiterals".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "unionWithLiterals".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_only_literals.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: union_with_only_literals.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funTaggedUnion".to_string(),
                    description: "Takes TaggedUnion".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "taggedUnionType".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tagged_union.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: tagged_union.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultNoTag".to_string(),
                    description: "Takes ResultLikeWithNoTag".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "eitherBothOptional".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_no_tag.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_no_tag.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultExact".to_string(),
                    description: "Takes ResultExact".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "either".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_exact.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_exact.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultLike".to_string(),
                    description: "Takes ResultLike".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "eitherOneOptional".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funResultLikeWithVoid".to_string(),
                    description: "Takes ResultLikeWithVoid".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "resultLikeWithVoid".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_void.clone(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: result_like_with_void.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBuiltinResultVS".to_string(),
                    description: "Takes Result<void, string>".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "result".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultVoidString"),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultVoidString"),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBuiltinResultSV".to_string(),
                    description: "Takes Result<string, void>".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "result".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringVoid"),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringVoid"),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funBuiltinResultSN".to_string(),
                    description: "Takes Result<string, number>".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "result".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringNumber"),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str().named("ResultStringNumber"),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funNoReturn".to_string(),
                    description: "Returns nothing".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "funArrowSync".to_string(),
                    description: "Arrow function returning text".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "text".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: str(),
                            }),
                        }],
                    }),
                    http_endpoint: vec![],
                },
            ],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
        // TestAgent from naming_extremes
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("TestAgent".to_string()),
            description: "TestAgent class".to_string(),
            constructor: AgentConstructor {
                name: Some("TestAgent".to_string()),
                description: "Constructor for TestAgent".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "name".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
            },
            methods: vec![
                AgentMethod {
                    name: "testAll".to_string(),
                    description: "Test all functionality".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "testString".to_string(),
                    description: "Test string functionality".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
                AgentMethod {
                    name: "testStruct".to_string(),
                    description: "Test struct functionality".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    http_endpoint: vec![],
                },
            ],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
        // StringAgent from naming_extremes
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("StringAgent".to_string()),
            description: "StringAgent class".to_string(),
            constructor: AgentConstructor {
                name: Some("StringAgent".to_string()),
                description: "Constructor for StringAgent".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "name".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
                        }),
                    }],
                }),
            },
            methods: vec![AgentMethod {
                name: "test".to_string(),
                description: "Test method".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                http_endpoint: vec![],
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
        // StructAgent from naming_extremes
        AgentType {
            type_name: golem_common::model::agent::AgentTypeName("StructAgent".to_string()),
            description: "StructAgent class".to_string(),
            constructor: AgentConstructor {
                name: Some("StructAgent".to_string()),
                description: "Constructor for StructAgent".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![NamedElementSchema {
                        name: "args".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: record(vec![
                                field("x", str()),
                                field("y", str()),
                                field("z", str()),
                            ])
                            .named("StructArgs"),
                        }),
                    }],
                }),
            },
            methods: vec![AgentMethod {
                name: "test".to_string(),
                description: "Test method".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                http_endpoint: vec![],
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        },
    ]
}
