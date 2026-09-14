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
use golem_cli::model::language::GuestLanguage;
use golem_common::model::Empty;
use golem_common::model::agent::{AgentConfigSource, AgentMode, AgentTypeName, Snapshotting};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, MetadataEnvelope,
    NamedField, NamedFieldType, OutputSchema, Role, SchemaGraph, SchemaType, SchemaTypeDef, TypeId,
    VariantCaseType,
};
use test_r::test;

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

/// A canonical multimodal schema type: `list<variant<…>>` whose list carries
/// the `Role::Multimodal` marker. Each modality case must declare a payload.
pub fn multimodal(cases: Vec<VariantCaseType>) -> SchemaType {
    let mut list = SchemaType::list(SchemaType::variant(cases));
    list.metadata_mut().role = Some(Role::Multimodal);
    list
}

/// A local (caller-overridable) agent config declaration.
pub fn local_config(path: Vec<&str>, value_type: SchemaType) -> AgentConfigDeclarationSchema {
    AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: path.into_iter().map(|s| s.to_string()).collect(),
        value_type,
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

/// Native guest streams with recursive items and runtime-dependent endpoint counts.
pub fn guest_streaming_agent_type(source_language: &str) -> AgentTypeSchema {
    let stream = |item| SchemaType::stream(Some(item));
    agent(
        "GuestStreamingAgent",
        source_language,
        vec![field("name", SchemaType::string())],
        vec![
            method(
                "consume",
                vec![field("items", stream(SchemaType::string()))],
                Some(SchemaType::u32()),
            ),
            method("produce", vec![], Some(stream(ref_to("StreamItem")))),
            method(
                "exchange",
                vec![
                    field("label", SchemaType::string()),
                    field("items", stream(ref_to("StreamItem"))),
                ],
                Some(ref_to("StreamBundle")),
            ),
            method(
                "forward",
                vec![field("bundle", ref_to("StreamBundle"))],
                Some(ref_to("StreamBundle")),
            ),
            method(
                "nested",
                vec![field("items", stream(stream(ref_to("StreamItem"))))],
                Some(stream(stream(ref_to("StreamItem")))),
            ),
            method(
                "recursive",
                vec![field("tree", ref_to("StreamTree"))],
                Some(ref_to("StreamTree")),
            ),
            method(
                "shapes",
                vec![
                    field("narrow", stream(SchemaType::s8())),
                    field("wide", stream(SchemaType::s32())),
                    field("list", stream(SchemaType::list(SchemaType::string()))),
                    field(
                        "fixed",
                        stream(SchemaType::fixed_list(SchemaType::string(), 2)),
                    ),
                    field(
                        "entries",
                        stream(SchemaType::map(SchemaType::string(), SchemaType::u32())),
                    ),
                    field(
                        "single",
                        stream(SchemaType::tuple(vec![SchemaType::string()])),
                    ),
                ],
                None,
            ),
            method("status", vec![], Some(SchemaType::string())),
        ],
        vec![
            def(
                "StreamItem",
                SchemaType::record(vec![
                    named_field("label", SchemaType::string()),
                    named_field("children", SchemaType::list(ref_to("StreamItem"))),
                ]),
            ),
            def(
                "StreamBundle",
                SchemaType::record(vec![
                    named_field("optional", SchemaType::option(stream(ref_to("StreamItem")))),
                    named_field("siblings", SchemaType::list(stream(ref_to("StreamItem")))),
                    named_field(
                        "named",
                        SchemaType::map(SchemaType::string(), stream(SchemaType::u32())),
                    ),
                    named_field(
                        "outcome",
                        SchemaType::result(golem_common::schema::ResultSpec {
                            ok: Some(Box::new(stream(ref_to("StreamItem")))),
                            err: Some(Box::new(SchemaType::string())),
                        }),
                    ),
                ]),
            ),
            def(
                "StreamTree",
                SchemaType::variant(vec![
                    variant_case("leaf", Some(stream(ref_to("StreamItem")))),
                    variant_case("branch", Some(SchemaType::list(ref_to("StreamTree")))),
                ]),
            ),
        ],
        AgentMode::Durable,
    )
}

#[test]
fn guest_streaming_fixture_classifies_recursive_methods() {
    let agent = guest_streaming_agent_type("rust");
    agent.validate().expect("valid guest streaming schema");
    for method in &agent.methods {
        assert_eq!(
            method.uses_streams(&agent.schema),
            method.name != "status",
            "incorrect stream classification for {}",
            method.name,
        );
    }
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
