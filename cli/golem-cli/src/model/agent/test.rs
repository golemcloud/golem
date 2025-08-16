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
    AgentConstructor, AgentMethod, AgentType, BinaryDescriptor, DataSchema, ElementSchema,
    NamedElementSchema, NamedElementSchemas, TextDescriptor,
};
use golem_wasm_ast::analysis::analysed_type::{
    case, field, list, option, r#enum, record, str, u32, unit_case, variant,
};

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
                            schema: ElementSchema::ComponentModel(person.clone()),
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
                        schema: ElementSchema::ComponentModel(location.clone()),
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
                        schema: ElementSchema::ComponentModel(list(person)),
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
                            schema: ElementSchema::ComponentModel(location),
                        },
                        NamedElementSchema {
                            name: "color".to_string(),
                            schema: ElementSchema::ComponentModel(color),
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
