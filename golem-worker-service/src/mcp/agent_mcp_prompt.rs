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

use golem_common::base_model::agent::{
    AgentConstructor, AgentMethod, AgentTypeName, DataSchema, ElementSchema, NamedElementSchema,
};
use rmcp::model::{
    GetPromptResult, Prompt, PromptMessage, PromptMessageContent, PromptMessageRole,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PromptName(pub String);

#[derive(Clone)]
pub struct AgentMcpPrompt {
    pub name: PromptName,
    pub prompt: Prompt,
    pub prompt_text: String,
}

impl AgentMcpPrompt {
    pub fn from_constructor_hint(
        agent_type_name: &AgentTypeName,
        description: &str,
        prompt_hint: &str,
    ) -> Self {
        let name = PromptName(agent_type_name.0.clone());

        let prompt = Prompt::new(name.0.clone(), Some(description.to_string()), None);

        AgentMcpPrompt {
            name,
            prompt,
            prompt_text: prompt_hint.to_string(),
        }
    }

    pub fn from_method_hint(
        agent_type_name: &AgentTypeName,
        method: &AgentMethod,
        constructor: &AgentConstructor,
        prompt_hint: &str,
    ) -> Self {
        let prompt_name = format!("{}-{}", agent_type_name.0, method.name);
        let name = PromptName(prompt_name.clone());

        let constructor_arg_names = constructor_arg_names(&constructor.input_schema);
        let input_description = describe_input(&constructor_arg_names, &method.input_schema);
        let output_description = describe_output(&method.output_schema);

        let mut text = prompt_hint.to_string();

        if let Some(input_desc) = input_description {
            text.push_str(&format!("\n\n{}", input_desc));
        }

        if let Some(output_desc) = output_description {
            text.push_str(&format!("\n\n{}", output_desc));
        }

        let prompt = Prompt::new(prompt_name, Some(method.description.clone()), None);

        AgentMcpPrompt {
            name,
            prompt,
            prompt_text: text,
        }
    }

    pub fn get_prompt_result(&self) -> GetPromptResult {
        GetPromptResult {
            description: self.prompt.description.clone(),
            messages: vec![PromptMessage {
                role: PromptMessageRole::User,
                content: PromptMessageContent::Text {
                    text: self.prompt_text.clone(),
                },
            }],
        }
    }
}

fn constructor_arg_names(schema: &DataSchema) -> Vec<String> {
    match schema {
        DataSchema::Tuple(schemas) => schemas.elements.iter().map(|e| e.name.clone()).collect(),
        DataSchema::Multimodal(_) => vec![],
    }
}

fn describe_input(
    constructor_arg_names: &[String],
    method_input_schema: &DataSchema,
) -> Option<String> {
    match method_input_schema {
        DataSchema::Tuple(schemas) => {
            let mut all_names: Vec<&str> =
                constructor_arg_names.iter().map(|s| s.as_str()).collect();
            all_names.extend(schemas.elements.iter().map(|e| e.name.as_str()));

            if all_names.is_empty() {
                None
            } else {
                Some(format!(
                    "Expected JSON input properties: {}",
                    all_names.join(", ")
                ))
            }
        }
        DataSchema::Multimodal(schemas) => {
            let part_names: Vec<&str> = schemas.elements.iter().map(|e| e.name.as_str()).collect();

            let mut all_property_names: Vec<&str> =
                constructor_arg_names.iter().map(|s| s.as_str()).collect();

            if !part_names.is_empty() {
                all_property_names.push("parts");
            }

            let mut desc = String::new();

            if !all_property_names.is_empty() {
                desc.push_str(&format!(
                    "Expected JSON input properties: {}",
                    all_property_names.join(", ")
                ));
            }

            if !part_names.is_empty() {
                desc.push_str(&format!(
                    "\n\nThe \"parts\" property is an array with elements named: {}",
                    part_names.join(", ")
                ));
            }

            if desc.is_empty() { None } else { Some(desc) }
        }
    }
}

fn describe_output_element(element: &NamedElementSchema) -> String {
    match &element.schema {
        ElementSchema::ComponentModel(_) => "JSON".to_string(),

        ElementSchema::UnstructuredText(text_desc) => {
            if let Some(desc) = &text_desc.restrictions {
                if desc.is_empty() {
                    return "text".to_string();
                }

                return format!(
                    "text with with one of the following language codes: {}",
                    desc.iter()
                        .map(|x| x.language_code.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            "text".to_string()
        }
        ElementSchema::UnstructuredBinary(binary) => {
            if let Some(restrictions) = &binary.restrictions {
                if restrictions.is_empty() {
                    return "binary".to_string();
                }

                return format!(
                    "binary with one of the following mime-types: {}",
                    restrictions
                        .iter()
                        .map(|x| x.mime_type.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            "binary".to_string()
        }
    }
}

fn describe_output(schema: &DataSchema) -> Option<String> {
    match schema {
        DataSchema::Tuple(schemas) => match schemas.elements.as_slice() {
            [] => None,
            [single] => Some(format!("output hint: {}", describe_output_element(single))),
            _ => None,
        },
        DataSchema::Multimodal(schemas) => {
            if schemas.elements.is_empty() {
                return None;
            }
            let parts: Vec<String> = schemas
                .elements
                .iter()
                .map(describe_output_element)
                .collect();
            Some(format!(
                "output hint: multimodal response: {}",
                parts.join(", ")
            ))
        }
    }
}

#[derive(Clone, Default)]
pub struct PromptRegistry {
    prompts: HashMap<PromptName, AgentMcpPrompt>,
}

impl PromptRegistry {
    pub fn insert(&mut self, prompt: AgentMcpPrompt) {
        self.prompts.insert(prompt.name.clone(), prompt);
    }

    pub fn list_prompts(&self) -> Vec<Prompt> {
        self.prompts.values().map(|p| p.prompt.clone()).collect()
    }

    pub fn get_by_name(&self, name: &str) -> Option<&AgentMcpPrompt> {
        self.prompts.get(&PromptName(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::agent::{
        BinaryDescriptor, ComponentModelElementSchema, DataSchema, NamedElementSchema,
        NamedElementSchemas, TextDescriptor,
    };
    use golem_wasm::analysis::analysed_type::str;
    use test_r::test;

    #[test]
    fn creates_prompt_from_constructor_hint() {
        let prompt = AgentMcpPrompt::from_constructor_hint(
            &AgentTypeName("WeatherAgent".to_string()),
            "A weather agent",
            "You are a weather assistant. Help the user get weather information.",
        );

        assert_eq!(prompt.name, PromptName("WeatherAgent".to_string()));
        assert_eq!(prompt.prompt.name, "WeatherAgent");
        assert_eq!(
            prompt.prompt.description.as_deref(),
            Some("A weather agent")
        );
        assert!(prompt.prompt.arguments.is_none());
        assert_eq!(
            prompt.prompt_text,
            "You are a weather assistant. Help the user get weather information."
        );
    }

    #[test]
    fn get_prompt_result_returns_user_message_with_hint() {
        let prompt = AgentMcpPrompt::from_constructor_hint(
            &AgentTypeName("MyAgent".to_string()),
            "desc",
            "Do something useful",
        );

        let result = prompt.get_prompt_result();
        assert_eq!(result.messages.len(), 1);
        assert!(matches!(result.messages[0].role, PromptMessageRole::User));
        match &result.messages[0].content {
            PromptMessageContent::Text { text } => {
                assert_eq!(text, "Do something useful");
            }
            _ => panic!("expected text content"),
        }
    }

    #[test]
    fn registry_insert_and_list() {
        let mut registry = PromptRegistry::default();
        registry.insert(AgentMcpPrompt::from_constructor_hint(
            &AgentTypeName("A".to_string()),
            "Agent A",
            "hint a",
        ));
        registry.insert(AgentMcpPrompt::from_constructor_hint(
            &AgentTypeName("B".to_string()),
            "Agent B",
            "hint b",
        ));

        let listed = registry.list_prompts();
        assert_eq!(listed.len(), 2);

        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
    }

    #[test]
    fn registry_get_by_name() {
        let mut registry = PromptRegistry::default();
        registry.insert(AgentMcpPrompt::from_constructor_hint(
            &AgentTypeName("X".to_string()),
            "Agent X",
            "hint x",
        ));

        assert!(registry.get_by_name("X").is_some());
        assert!(registry.get_by_name("Y").is_none());
    }

    #[test]
    fn method_prompt_name_is_agent_type_dash_method() {
        let constructor = AgentConstructor {
            name: None,
            description: "ctor".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "city".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: str(),
                    }),
                }],
            }),
        };
        let method = AgentMethod {
            name: "get_weather".to_string(),
            description: "Gets weather".to_string(),
            prompt_hint: Some("Fetch the weather".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "location".to_string(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                }],
            }),
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "report".to_string(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                }],
            }),
            http_endpoint: vec![],
        };

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("WeatherAgent".to_string()),
            &method,
            &constructor,
            "Fetch the weather",
        );

        assert_eq!(
            prompt.name,
            PromptName("WeatherAgent-get_weather".to_string())
        );
        assert_eq!(prompt.prompt.name, "WeatherAgent-get_weather");
        assert_eq!(prompt.prompt.description.as_deref(), Some("Gets weather"));
    }

    #[test]
    fn method_prompt_text_includes_constructor_and_method_args_and_output() {
        let constructor = AgentConstructor {
            name: None,
            description: "ctor".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "tenant".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: str(),
                    }),
                }],
            }),
        };
        let method = AgentMethod {
            name: "analyze".to_string(),
            description: "Analyze data".to_string(),
            prompt_hint: Some("Run analysis".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "query".to_string(),
                        schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                            element_type: str(),
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
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "result".to_string(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                }],
            }),
            http_endpoint: vec![],
        };

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("DataAgent".to_string()),
            &method,
            &constructor,
            "Run analysis",
        );

        assert!(prompt.prompt_text.starts_with("Run analysis"));
        assert!(
            prompt
                .prompt_text
                .contains("Expected JSON input properties: tenant, query, image"),
            "got: {}",
            prompt.prompt_text
        );
        assert!(prompt.prompt_text.contains("Output: result: text"));
    }

    #[test]
    fn method_prompt_multimodal_input_describes_parts_array() {
        let constructor = AgentConstructor {
            name: None,
            description: "ctor".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "tenant".to_string(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: str(),
                    }),
                }],
            }),
        };
        let method = AgentMethod {
            name: "process".to_string(),
            description: "Process multimodal".to_string(),
            prompt_hint: Some("Process it".to_string()),
            input_schema: DataSchema::Multimodal(NamedElementSchemas {
                elements: vec![
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
            output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            http_endpoint: vec![],
        };

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("MultiAgent".to_string()),
            &method,
            &constructor,
            "Process it",
        );

        assert!(
            prompt
                .prompt_text
                .contains("Expected JSON input properties: tenant, parts"),
            "got: {}",
            prompt.prompt_text
        );
        assert!(
            prompt.prompt_text.contains(
                "The \"parts\" property is an array with elements named: description, photo"
            ),
            "got: {}",
            prompt.prompt_text
        );
    }

    #[test]
    fn method_prompt_omits_empty_sections() {
        let constructor = AgentConstructor {
            name: None,
            description: "ctor".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        };
        let method = AgentMethod {
            name: "ping".to_string(),
            description: "Ping".to_string(),
            prompt_hint: Some("Just ping".to_string()),
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            output_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
            http_endpoint: vec![],
        };

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("PingAgent".to_string()),
            &method,
            &constructor,
            "Just ping",
        );

        assert_eq!(prompt.prompt_text, "Just ping");
    }
}
