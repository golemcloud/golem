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
    AgentConstructor, AgentMethod, AgentType, BinaryDescriptor, ComponentModelElementSchema,
    DataSchema, ElementSchema, NamedElementSchema, NamedElementSchemas, TextDescriptor,
};
use golem_wasm::analysis::analysed_type::{
    case, field, list, option, r#enum, record, result, s32, str, u32, u8, unit_case, variant,
};

pub fn single_agent_wrapper_types() -> Vec<AgentType> {
    vec![AgentType {
        type_name: "agent1".to_string(),
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
            },
        ],
        dependencies: vec![],
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
            type_name: "agent1".to_string(),
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
            }],
            dependencies: vec![],
        },
        AgentType {
            type_name: "agent2".to_string(),
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
            }],
            dependencies: vec![],
        },
    ];

    agent_types
}

pub fn agent_type_with_wit_keywords() -> Vec<AgentType> {
    vec![AgentType {
        type_name: "agent1".to_string(),
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
            },
        ],
        dependencies: vec![],
    }]
}

pub fn reproducer_for_multiple_types_called_element() -> Vec<AgentType> {
    vec![
        AgentType {
            type_name: "assistant-agent".to_string(),
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
            }],
            dependencies: vec![],
        },
        AgentType {
            type_name: "weather-agent".to_string(),
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
            }],
            dependencies: vec![],
        },
    ]
}

pub fn reproducer_for_issue_with_enums() -> Vec<AgentType> {
    vec![AgentType {
        type_name: "foo-agent".to_string(),
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
        }],
        dependencies: vec![],
    }]
}

pub fn reproducer_for_issue_with_result_types() -> Vec<AgentType> {
    vec![AgentType {
        type_name: "bar-agent".to_string(),
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
        }],
        dependencies: vec![],
    }]
}

pub fn multimodal_untagged_variant_in_out() -> Vec<AgentType> {
    vec![AgentType {
        type_name: "test-agent".to_string(),
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
        }],
        dependencies: vec![],
    }]
}
