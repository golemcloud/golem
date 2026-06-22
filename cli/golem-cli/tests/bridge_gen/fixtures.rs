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

use crate::workspace_path;
use golem_cli::model::GuestLanguage;
use golem_common::model::Empty;
use golem_common::model::agent::{AgentMode, AgentTypeName, Snapshotting};
use golem_common::schema::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, MetadataEnvelope,
    NamedField, NamedFieldType, OutputSchema, SchemaGraph, SchemaType, SchemaTypeDef, TypeId,
    VariantCaseType,
};

pub fn field(name: impl Into<String>, schema: SchemaType) -> NamedField {
    NamedField::user_supplied(name, schema)
}

pub fn named_field(name: impl Into<String>, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.into(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

pub fn variant_case(name: impl Into<String>, payload: Option<SchemaType>) -> VariantCaseType {
    VariantCaseType {
        name: name.into(),
        payload,
        metadata: MetadataEnvelope::default(),
    }
}

pub fn method(
    name: impl Into<String>,
    input: Vec<NamedField>,
    output: Option<SchemaType>,
) -> AgentMethodSchema {
    AgentMethodSchema {
        name: name.into(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters(input),
        output_schema: output
            .map(|t| OutputSchema::Single(Box::new(t)))
            .unwrap_or(OutputSchema::Unit),
        http_endpoint: Vec::new(),
        read_only: None,
    }
}

pub fn agent(
    type_name: impl Into<String>,
    source_language: impl Into<String>,
    constructor: Vec<NamedField>,
    methods: Vec<AgentMethodSchema>,
    defs: Vec<SchemaTypeDef>,
    mode: AgentMode,
) -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: AgentTypeName(type_name.into()),
        description: "An example agent".to_string(),
        source_language: source_language.into(),
        schema: SchemaGraph {
            defs,
            root: SchemaType::record(vec![]),
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: "Creates an example agent instance".into(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(constructor),
        },
        methods,
        dependencies: vec![],
        mode,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: Vec::new(),
    }
}

pub fn def(id: impl Into<String>, body: SchemaType) -> SchemaTypeDef {
    SchemaTypeDef {
        id: TypeId::new(id),
        name: None,
        body,
    }
}

pub fn ref_to(id: impl Into<String>) -> SchemaType {
    SchemaType::ref_to(TypeId::new(id))
}

pub fn single_agent_wrapper_types() -> Vec<AgentTypeSchema> {
    vec![agent(
        "agent1",
        "",
        vec![
            field("a", SchemaType::u32()),
            field("b", SchemaType::option(SchemaType::string())),
        ],
        vec![
            method("f1", vec![], Some(SchemaType::string())),
            method(
                "f2",
                vec![field("x", SchemaType::u32()), field("y", SchemaType::u32())],
                Some(SchemaType::u32()),
            ),
        ],
        vec![],
        AgentMode::Durable,
    )]
}

pub fn multi_agent_wrapper_2_types() -> Vec<AgentTypeSchema> {
    let color = def(
        "color",
        SchemaType::r#enum(vec!["red".into(), "green".into(), "blue".into()]),
    );
    let person = def(
        "person",
        SchemaType::record(vec![
            named_field("first-name", SchemaType::string()),
            named_field("last-name", SchemaType::string()),
            named_field("age", SchemaType::option(SchemaType::u32())),
            named_field("eye-color", ref_to("color")),
        ]),
    );
    let location = def(
        "location",
        SchemaType::variant(vec![
            variant_case("home", Some(SchemaType::string())),
            variant_case("work", Some(SchemaType::string())),
            variant_case("unknown", None),
        ]),
    );

    vec![
        agent(
            "agent1",
            "",
            vec![
                field("person", ref_to("person")),
                field("description", SchemaType::string()),
                field("photo", SchemaType::list(SchemaType::u8())),
            ],
            vec![method("f1", vec![], Some(ref_to("location")))],
            vec![color.clone(), person.clone(), location.clone()],
            AgentMode::Durable,
        ),
        agent(
            "agent2",
            "",
            vec![field("person-group", SchemaType::list(ref_to("person")))],
            vec![method(
                "f2",
                vec![
                    field("place", ref_to("location")),
                    field("color", ref_to("color")),
                ],
                Some(SchemaType::string()),
            )],
            vec![color, person, location],
            AgentMode::Durable,
        ),
    ]
}

#[allow(dead_code)]
pub fn code_first_snippets_agent_types(language: GuestLanguage) -> Vec<AgentTypeSchema> {
    let goldenfile = workspace_path()
        .join("cli/golem-cli/test-data/goldenfiles/extracted-agent-types")
        .join(format!("code_first_snippets_{}.json", language.id()));

    serde_json::from_str(&std::fs::read_to_string(&goldenfile).unwrap()).unwrap_or_else(|err| {
        panic!(
            "Failed to deserialize golden file {}: {err}",
            goldenfile.display()
        )
    })
}

pub fn code_first_snippets_agent_type(
    language: GuestLanguage,
    agent_name: &str,
) -> AgentTypeSchema {
    code_first_snippets_agent_types(language)
        .into_iter()
        .find(|t| t.type_name.0 == agent_name)
        .unwrap_or_else(|| {
            panic!(
                "Agent type {agent_name} not found in {language} extracted code first snippets goldenfile"
            )
        })
}
