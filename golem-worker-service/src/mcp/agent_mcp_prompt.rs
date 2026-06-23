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

use crate::mcp::schema::field_name_mapping;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, FieldSource, InputSchema, NamedField, OutputSchema,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::multimodal::{is_multimodal_schema_type, multimodal_variant_cases};
use golem_common::schema::schema_type::SchemaType;
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
        graph: &SchemaGraph,
        method: &AgentMethodSchema,
        constructor: &AgentConstructorSchema,
        prompt_hint: &str,
    ) -> Self {
        let prompt_name = format!("{}-{}", agent_type_name.0, method.name);
        let name = PromptName(prompt_name.clone());

        // Mirror the advertised tool schema: constructor/method parameter names
        // that collide are disambiguated, so the prompt must list the same
        // (possibly prefixed) property names a client will actually send.
        let mapping = field_name_mapping(constructor, method);
        let constructor_fields =
            mapping.apply_to_constructor_fields(constructor.input_schema.fields());
        let method_input_schema =
            InputSchema::Parameters(mapping.apply_to_method_fields(method.input_schema.fields()));

        let constructor_arg_names = constructor_arg_names(&constructor_fields);
        let input_description = describe_input(graph, &constructor_arg_names, &method_input_schema);
        let output_description = describe_output(graph, &method.output_schema);

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

fn constructor_arg_names(fields: &[NamedField]) -> Vec<String> {
    fields
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .map(|f| f.name.clone())
        .collect()
}

fn describe_input(
    graph: &SchemaGraph,
    constructor_arg_names: &[String],
    method_input_schema: &InputSchema,
) -> Option<String> {
    let user_fields: Vec<&NamedField> = method_input_schema
        .fields()
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .collect();

    // A multimodal method input is carried as a single user field (`parts`)
    // whose schema is the structural form `list<variant<… Role::Multimodal>>`.
    let multimodal_field = user_fields
        .iter()
        .find(|f| is_multimodal_schema_type(graph, &f.schema).unwrap_or(false));

    if let Some(field) = multimodal_field {
        let part_names: Vec<String> = multimodal_variant_cases(graph, &field.schema)
            .ok()
            .flatten()
            .map(|cases| cases.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();

        let mut all_property_names: Vec<String> = constructor_arg_names.to_vec();
        if !part_names.is_empty() {
            all_property_names.push(field.name.clone());
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
                "\n\nThe \"{}\" property is an array with elements named: {}",
                field.name,
                part_names.join(", ")
            ));
        }

        if desc.is_empty() { None } else { Some(desc) }
    } else {
        let mut all_names: Vec<String> = constructor_arg_names.to_vec();
        all_names.extend(user_fields.iter().map(|f| f.name.clone()));

        if all_names.is_empty() {
            None
        } else {
            Some(format!(
                "Expected JSON input properties: {}",
                all_names.join(", ")
            ))
        }
    }
}

fn describe_output_type(graph: &SchemaGraph, ty: &SchemaType) -> String {
    match graph.resolve_ref(ty) {
        Ok(SchemaType::Text { restrictions, .. }) => match &restrictions.languages {
            Some(langs) if !langs.is_empty() => format!(
                "text with one of the following language codes: {}",
                langs.join(", ")
            ),
            _ => "text".to_string(),
        },
        Ok(SchemaType::Binary { restrictions, .. }) => match &restrictions.mime_types {
            Some(mimes) if !mimes.is_empty() => format!(
                "binary with one of the following mime-types: {}",
                mimes.join(", ")
            ),
            _ => "binary".to_string(),
        },
        _ => "JSON".to_string(),
    }
}

fn describe_output(graph: &SchemaGraph, schema: &OutputSchema) -> Option<String> {
    match schema {
        OutputSchema::Unit => None,
        OutputSchema::Single(ty) => {
            if let Some(cases) = multimodal_variant_cases(graph, ty).ok().flatten() {
                if cases.is_empty() {
                    return None;
                }
                let parts: Vec<String> = cases
                    .iter()
                    .filter_map(|c| c.payload.as_ref())
                    .map(|body| describe_output_type(graph, body))
                    .collect();
                Some(format!(
                    "output hint: multimodal response: {}",
                    parts.join(", ")
                ))
            } else {
                Some(format!("output hint: {}", describe_output_type(graph, ty)))
            }
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
    use golem_common::schema::metadata::Role;
    use golem_common::schema::schema_type::{
        BinaryRestrictions, TextRestrictions, VariantCaseType,
    };
    use test_r::test;

    fn constructor(fields: Vec<NamedField>) -> AgentConstructorSchema {
        AgentConstructorSchema {
            name: None,
            description: "ctor".to_string(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(fields),
        }
    }

    fn method(
        name: &str,
        description: &str,
        input: InputSchema,
        output: OutputSchema,
    ) -> AgentMethodSchema {
        AgentMethodSchema {
            name: name.to_string(),
            description: description.to_string(),
            prompt_hint: None,
            input_schema: input,
            output_schema: output,
            http_endpoint: vec![],
            read_only: None,
        }
    }

    /// Build the structural multimodal input form: a single user-supplied
    /// `parts` field whose schema is `list<variant<…>>` with the list node
    /// carrying `Role::Multimodal`.
    fn multimodal_parts_field(branches: Vec<(&str, SchemaType)>) -> NamedField {
        let variant = SchemaType::variant(
            branches
                .into_iter()
                .map(|(name, body)| VariantCaseType {
                    name: name.to_string(),
                    payload: Some(body),
                    metadata: Default::default(),
                })
                .collect(),
        );
        let mut list = SchemaType::list(variant);
        list.metadata_mut().role = Some(Role::Multimodal);
        NamedField::user_supplied("parts", list)
    }

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
        let graph = SchemaGraph::empty();
        let constructor = constructor(vec![NamedField::user_supplied(
            "city",
            SchemaType::string(),
        )]);
        let method = method(
            "get_weather",
            "Gets weather",
            InputSchema::Parameters(vec![NamedField::user_supplied(
                "location",
                SchemaType::text(TextRestrictions::default()),
            )]),
            OutputSchema::Single(Box::new(SchemaType::text(TextRestrictions::default()))),
        );

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("WeatherAgent".to_string()),
            &graph,
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
        let graph = SchemaGraph::empty();
        let constructor = constructor(vec![NamedField::user_supplied(
            "tenant",
            SchemaType::string(),
        )]);
        let method = method(
            "analyze",
            "Analyze data",
            InputSchema::Parameters(vec![
                NamedField::user_supplied("query", SchemaType::string()),
                NamedField::user_supplied(
                    "image",
                    SchemaType::binary(BinaryRestrictions::default()),
                ),
            ]),
            OutputSchema::Single(Box::new(SchemaType::text(TextRestrictions::default()))),
        );

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("DataAgent".to_string()),
            &graph,
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

        assert!(prompt.prompt_text.contains("text"));
    }

    #[test]
    fn method_prompt_multimodal_input_describes_parts_array() {
        let graph = SchemaGraph::empty();
        let constructor = constructor(vec![NamedField::user_supplied(
            "tenant",
            SchemaType::string(),
        )]);
        let method = method(
            "process",
            "Process multimodal",
            InputSchema::Parameters(vec![multimodal_parts_field(vec![
                ("description", SchemaType::text(TextRestrictions::default())),
                ("photo", SchemaType::binary(BinaryRestrictions::default())),
            ])]),
            OutputSchema::Unit,
        );

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("MultiAgent".to_string()),
            &graph,
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
        let graph = SchemaGraph::empty();
        let constructor = constructor(vec![]);
        let method = method(
            "ping",
            "Ping",
            InputSchema::Parameters(vec![]),
            OutputSchema::Unit,
        );

        let prompt = AgentMcpPrompt::from_method_hint(
            &AgentTypeName("PingAgent".to_string()),
            &graph,
            &method,
            &constructor,
            "Just ping",
        );

        assert_eq!(prompt.prompt_text, "Just ping");
    }
}
