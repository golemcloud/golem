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

use super::*;
use crate::base_model::Empty;
use crate::base_model::agent::{
    AgentHttpAuthDetails, AgentTypeName, CorsOptions, HttpEndpointDetails, LiteralSegment,
    PathVariable, SnapshottingConfig,
};
use crate::schema::agent::{
    AgentConfigDeclarationSchema, AgentConstructorSchema, AgentDependencySchema, AutoInjectedKind,
    InputSchema, NamedField, OutputSchema,
};
use crate::schema::{NamedFieldType, SchemaTypeDef, TypeId};
use serde_json::Value;
use test_r::test;

fn corpus() -> Vec<Value> {
    let value: Value = serde_json::from_str(include_str!(
        "../../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    value["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["suite"] == "metadata")
        .cloned()
        .collect()
}

// Build the fixture's named H01 types independently of the production builders.
fn fixture_schema(name: &str, input: &Value) -> SchemaType {
    let fields: &[(&str, &str)] = match name {
        "Header" => &[("name", "string"), ("value", "list<u8>")],
        "HttpRequest" => &[
            ("method", "string"),
            ("scheme", "string"),
            ("authority", "string"),
            ("path", "string"),
            ("query", "option<string>"),
            ("headers", "list<Header>"),
            ("body", "stream<list<u8>>"),
        ],
        "HttpResponse" => &[
            ("status", "u16"),
            ("headers", "list<Header>"),
            ("body", "stream<list<u8>>"),
        ],
        "string" => return SchemaType::string(),
        "u8" => return SchemaType::u8(),
        "u16" => return SchemaType::u16(),
        other => {
            for (prefix, kind) in [("list<", 0), ("stream<", 1), ("option<", 2)] {
                if let Some(inner) = other
                    .strip_prefix(prefix)
                    .and_then(|name| name.strip_suffix('>'))
                {
                    let inner = fixture_schema(inner, input);
                    return match kind {
                        0 => SchemaType::list(inner),
                        1 => SchemaType::stream(Some(inner)),
                        _ => SchemaType::option(inner),
                    };
                }
            }
            assert!(
                input["schema_aliases"].get(other).is_some(),
                "unknown fixture schema {other}"
            );
            return SchemaType::ref_to(TypeId::new(other));
        }
    };
    SchemaType::record(
        fields
            .iter()
            .map(|(field, ty)| NamedFieldType {
                name: (*field).into(),
                body: fixture_schema(
                    input["schema_overrides"][name][*field]
                        .as_str()
                        .unwrap_or(ty),
                    input,
                ),
                metadata: Default::default(),
            })
            .collect(),
    )
}

fn parameters(value: &Value, input: &Value) -> InputSchema {
    InputSchema::parameters(value.as_array().into_iter().flatten().map(|pair| {
        NamedField::user_supplied(
            pair[0].as_str().unwrap(),
            fixture_schema(pair[1].as_str().unwrap(), input),
        )
    }))
}

fn path(value: &str) -> Vec<PathSegment> {
    if value == "/" {
        return vec![];
    }
    value
        .strip_prefix('/')
        .unwrap()
        .split('/')
        .map(|segment| {
            if let Some(name) = segment
                .strip_prefix('{')
                .and_then(|segment| segment.strip_suffix('}'))
            {
                PathSegment::PathVariable(PathVariable {
                    variable_name: name.into(),
                })
            } else {
                PathSegment::Literal(LiteralSegment {
                    value: segment.into(),
                })
            }
        })
        .collect()
}

fn mappings(value: &Value) -> Vec<FileMapping> {
    FileMapping::compile_list(
        value
            .as_array()
            .into_iter()
            .flatten()
            .map(|pair| (pair[0].as_str().unwrap(), pair[1].as_str().unwrap())),
    )
    .unwrap()
}

fn agent(input: &Value) -> AgentTypeSchema {
    let constructor = AgentConstructorSchema {
        name: None,
        description: String::new(),
        prompt_hint: None,
        input_schema: parameters(&input["constructor"], input),
    };
    let mounts = input["mounts"].as_array().unwrap();
    assert_eq!(
        mounts.len(),
        1,
        "fixture mount count needs explicit representation"
    );
    AgentTypeSchema {
        type_name: AgentTypeName("fixture".into()),
        kind: serde_json::from_value(input["kind"].clone()).unwrap(),
        description: String::new(),
        source_language: "fixture".into(),
        schema: SchemaGraph {
            root: SchemaGraph::empty().root,
            defs: input["schema_aliases"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, target)| SchemaTypeDef {
                    id: TypeId::new(name),
                    name: Some(name.clone()),
                    body: fixture_schema(target.as_str().unwrap(), input),
                })
                .collect(),
        },
        constructor,
        methods: input["methods"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|method| AgentMethodSchema {
                name: method["name"].as_str().unwrap().into(),
                description: String::new(),
                prompt_hint: None,
                input_schema: parameters(&method["input"], input),
                output_schema: OutputSchema::Single(Box::new(fixture_schema(
                    method["output"].as_str().unwrap(),
                    input,
                ))),
                http_endpoint: method["bindings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|binding| HttpEndpointDetails {
                        http_method: match binding[0].as_str().unwrap() {
                            "Any" => HttpMethod::Any(Empty {}),
                            "GET" => HttpMethod::Get(Empty {}),
                            other => panic!("unsupported fixture verb {other}"),
                        },
                        path_suffix: path(binding[1].as_str().unwrap()),
                        header_vars: vec![],
                        query_vars: vec![],
                        auth_details: method
                            .get("endpoint_auth")
                            .map(|_| AgentHttpAuthDetails { required: true }),
                        cors_options: CorsOptions {
                            allowed_patterns: vec![],
                        },
                        durable_streams: None,
                    })
                    .collect(),
                read_only: None,
            })
            .collect(),
        dependencies: input["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|name| AgentDependencySchema {
                type_name: name.as_str().unwrap().into(),
                description: None,
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: InputSchema::parameters([]),
                },
                methods: vec![],
            })
            .collect(),
        config: input["config"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|name| AgentConfigDeclarationSchema {
                source: crate::base_model::agent::AgentConfigSource::Local,
                path: vec![name.as_str().unwrap().into()],
                value_type: SchemaType::string(),
            })
            .collect(),
        mode: match input["mode"].as_str().unwrap() {
            "ephemeral" => AgentMode::Ephemeral,
            "durable" => AgentMode::Durable,
            other => panic!("unknown mode {other}"),
        },
        snapshotting: if input["snapshot"] == true {
            Snapshotting::Enabled(SnapshottingConfig::Default(Empty {}))
        } else {
            Snapshotting::Disabled(Empty {})
        },
        http_mount: Some(HttpMountDetails {
            path_prefix: path(mounts[0].as_str().unwrap()),
            auth_details: None,
            phantom_agent: input["phantom"] == true,
            cors_options: CorsOptions {
                allowed_patterns: vec![],
            },
            webhook_suffix: vec![],
            static_bindings: mappings(&input["static_bindings"]),
            filesystem_bindings: mappings(&input["filesystem_bindings"]),
            openapi_provider_method: input["provider"].as_str().map(str::to_string),
        }),
    }
}

fn from_case(id: &str) -> AgentTypeSchema {
    agent(&corpus().into_iter().find(|case| case["id"] == id).unwrap()["input"])
}

fn corpus_category(error: &HttpAgentValidationError) -> &'static str {
    match error {
        HttpAgentValidationError::DuplicateMethod(_) => "duplicate-method",
        HttpAgentValidationError::InvalidFileMapping(_) => "file-mapping",
        HttpAgentValidationError::RouterMethodOnRegularAgent(_)
        | HttpAgentValidationError::RouterMethodRole(_) => "router-method-role",
        HttpAgentValidationError::StaticBindingsOnRegularAgent => "static-owner",
        HttpAgentValidationError::OpenApiProviderOnRegularAgent => "provider-owner",
        HttpAgentValidationError::RouterMode => "router-mode",
        HttpAgentValidationError::RouterConstructor => "router-constructor",
        HttpAgentValidationError::RouterSnapshot => "router-snapshot",
        HttpAgentValidationError::RouterMount => "router-mount",
        HttpAgentValidationError::FilesystemOwner => "filesystem-owner",
        HttpAgentValidationError::ProviderSchema(_) => "provider-schema",
        HttpAgentValidationError::HandlerEndpointPolicy(_) => "handler-endpoint-policy",
        HttpAgentValidationError::HandlerSchema(_) => "handler-schema",
        HttpAgentValidationError::UnboundConstructor(_) => "unbound-constructor",
    }
}

#[test]
fn shared_metadata_corpus() {
    for case in corpus() {
        let agent = agent(&case["input"]);
        let result = validate(&agent);
        if let Some(error) = case["expect"]["error"].as_str() {
            let actual = result.unwrap_err();
            assert_eq!(corpus_category(&actual), error, "{}: {actual}", case["id"]);
        } else {
            assert!(result.is_ok(), "{}: {result:?}", case["id"]);
            if let Some(expected) = case["expect"].get("handler") {
                let handler = agent
                    .methods
                    .iter()
                    .find(|method| !method.http_endpoint.is_empty())
                    .map(|method| method.name.as_str());
                assert_eq!(
                    serde_json::to_value(handler).unwrap(),
                    *expected,
                    "{}",
                    case["id"]
                );
            }
            if let Some(expected) = case["expect"].get("provider") {
                assert_eq!(
                    serde_json::to_value(
                        &agent.http_mount.as_ref().unwrap().openapi_provider_method
                    )
                    .unwrap(),
                    *expected,
                    "{}",
                    case["id"]
                );
            }
        }
    }
}

#[test]
fn router_roles_are_distinct_and_closed() {
    let router = from_case("metadata-arbitrary-handler-name");
    let mut both = router.clone();
    let provider = from_case("metadata-provider-only");
    both.methods.extend(provider.methods);
    both.http_mount.as_mut().unwrap().openapi_provider_method = Some("describe".into());
    both.validate().unwrap();
    both.http_mount.as_mut().unwrap().openapi_provider_method = Some("serveWhatever".into());
    assert!(both.validate().is_err());
    let mut duplicate = router.clone();
    duplicate.methods.push(duplicate.methods[0].clone());
    assert!(duplicate.validate().is_err());
    let mut empty = router.clone();
    empty.methods.clear();
    empty.validate().unwrap();
    empty.http_mount = None;
    assert!(empty.validate().is_err());
    let mut extra_binding = router.clone();
    extra_binding.methods[0]
        .http_endpoint
        .push(router.methods[0].http_endpoint[0].clone());
    assert!(extra_binding.validate().is_err());
    let mut wrong_name = router;
    let InputSchema::Parameters(fields) = &mut wrong_name.methods[0].input_schema;
    fields[0].name = "input".into();
    assert!(wrong_name.validate().is_err());
}

#[test]
fn principal_allowed_only_alongside_handler_request() {
    let principal = NamedField::auto_injected(
        "principal",
        AutoInjectedKind::Principal,
        SchemaType::string(),
    );
    let mut router = from_case("metadata-arbitrary-handler-name");
    let InputSchema::Parameters(fields) = &mut router.methods[0].input_schema;
    fields.insert(0, principal.clone());
    router.validate().unwrap();
    router.constructor.input_schema = InputSchema::parameters([principal.clone()]);
    assert!(router.validate().is_err());
    let mut provider = from_case("metadata-provider-only");
    provider.methods[0].input_schema = InputSchema::parameters([principal]);
    assert!(provider.validate().is_err());
}

#[test]
fn live_file_identity_must_be_fully_and_uniquely_path_bound() {
    let original = from_case("metadata-live-typed-overlap");
    for replacement in [vec![], path("/users/{other}"), path("/users/{id}/{id}")] {
        let mut agent = original.clone();
        agent.http_mount.as_mut().unwrap().path_prefix = replacement;
        assert!(agent.validate().is_err());
    }
    let mut agent = original.clone();
    agent.mode = AgentMode::Ephemeral;
    assert!(agent.validate().is_err());
    let mut agent = original;
    let InputSchema::Parameters(fields) = &mut agent.constructor.input_schema;
    fields[0].schema = SchemaType::list(SchemaType::string());
    assert!(agent.validate().is_err());
}
