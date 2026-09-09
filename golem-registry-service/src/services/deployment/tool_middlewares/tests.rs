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

use super::{
    compile_tool_middleware_chains, effective_installations, synthesize_effective_definition,
};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentRevision;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, ToolBindingInput, ToolFilesystemAccess, ToolName,
    ToolSource,
};
use golem_common::model::tool_middleware::{
    RegisteredToolMiddleware, ToolMiddlewareInstallation, ToolMiddlewareMergeMode,
    ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::schema::tool::compatibility::ToolCompatibilityMode;
use golem_common::schema::tool::{
    CommandBody, CommandIndex, CommandNode, CommandTree, Doc, ErrorCase, ErrorKind, Globals,
    MonomorphicToolMiddlewareScope, Positionals, Tool, ToolMiddleware, ToolMiddlewareScope,
};
use golem_common::schema::{NamedFieldType, SchemaGraph, SchemaType, SchemaTypeDef, TypeId};
use std::collections::BTreeMap;
use test_r::test;

fn error(name: &str, kind: ErrorKind, exit_code: u8, payload: SchemaType) -> ErrorCase {
    ErrorCase {
        name: name.to_string(),
        doc: Doc::default(),
        kind,
        exit_code,
        payload: Some(payload),
    }
}

fn tool(name: &str, error: ErrorCase, schema: SchemaGraph) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: name.to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: Some(CommandBody {
                    positionals: Positionals::default(),
                    options: Vec::new(),
                    flags: Vec::new(),
                    constraints: Vec::new(),
                    stdin: None,
                    stdout: None,
                    result: None,
                    errors: vec![error],
                    annotations: None,
                }),
            }],
        },
        schema,
    }
}

fn recursive_graph(id: &str, field_type: SchemaType) -> SchemaGraph {
    SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new(id),
            name: None,
            body: SchemaType::record(vec![NamedFieldType {
                name: "value".to_string(),
                body: field_type,
                metadata: Default::default(),
            }]),
        }],
        root: SchemaType::ref_to(TypeId::new(id)),
    }
}

fn empty_error(name: &str) -> ErrorCase {
    ErrorCase {
        name: name.to_string(),
        doc: Doc::default(),
        kind: ErrorKind::RuntimeError,
        exit_code: 1,
        payload: None,
    }
}

fn command(name: &str, errors: Vec<ErrorCase>) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        aliases: Vec::new(),
        doc: Doc::default(),
        globals: Globals::default(),
        subcommands: Vec::new(),
        body: Some(CommandBody {
            positionals: Positionals::default(),
            options: Vec::new(),
            flags: Vec::new(),
            constraints: Vec::new(),
            stdin: None,
            stdout: None,
            result: None,
            errors,
            annotations: None,
        }),
    }
}

fn simple_tool(name: &str) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![command(name, Vec::new())],
        },
        schema: SchemaGraph::empty(),
    }
}

fn installation(name: &str, value: i32) -> ToolMiddlewareInstallation {
    ToolMiddlewareInstallation {
        name: ToolMiddlewareName::try_from(name).unwrap(),
        version: Some("2.0.0".to_string()),
        parameters: NormalizedJsonValue::new(serde_json::json!({ "value": value })),
        account: Some("middleware@example.com".into()),
        filesystem_access: ToolFilesystemAccess::Denied,
    }
}

fn registered_middleware(name: &str, scope: ToolMiddlewareScope) -> RegisteredToolMiddleware {
    RegisteredToolMiddleware {
        deployment_revision: DeploymentRevision::INITIAL,
        release_id: None,
        definition: ToolMiddleware {
            name: name.to_string(),
            version: "2.0.0".to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            scope,
        },
        provision: Default::default(),
        source: ToolMiddlewareSource::Component {
            component_id: Default::default(),
            component_revision: ComponentRevision::INITIAL,
            component_name: "middleware:component".try_into().unwrap(),
        },
        owner_account_id: Default::default(),
        owner_account_email: "middleware@example.com".into(),
        metadata_version: "pinned-metadata".to_string(),
        metadata_digest: Default::default(),
    }
}

struct CompilerFixture {
    tool: RegisteredTool,
    binding: CompiledToolBinding,
    agent: AgentTypeName,
    tool_name: ToolName,
}

impl CompilerFixture {
    fn new() -> Self {
        let definition = simple_tool("leaf");
        let source = ToolSource::Component {
            component_id: Default::default(),
            component_revision: ComponentRevision::INITIAL,
            component_name: "tool:component".try_into().unwrap(),
        };
        let agent: AgentTypeName = "agent".parse().unwrap();
        let tool_name = ToolName::try_from("leaf").unwrap();
        Self {
            tool: RegisteredTool {
                deployment_revision: DeploymentRevision::INITIAL,
                release_id: None,
                definition,
                provision: Default::default(),
                source: source.clone(),
                owner_account_id: Default::default(),
                owner_account_email: "tool@example.com".into(),
                metadata_version: "tool-metadata".to_string(),
                metadata_digest: Default::default(),
            },
            binding: CompiledToolBinding {
                deployment_revision: DeploymentRevision::INITIAL,
                release_id: None,
                agent_type_name: agent.clone(),
                tool_name: tool_name.clone(),
                version: "1.0.0".to_string(),
                metadata_version: "tool-metadata".to_string(),
                metadata_digest: Default::default(),
                account_id: Default::default(),
                account_email: "tool@example.com".into(),
                parameters: NormalizedJsonValue::new(serde_json::Value::Null),
                secret_keys_readable: Default::default(),
                secret_keys_revealable: Default::default(),
                filesystem_access: ToolFilesystemAccess::Denied,
                source,
            },
            agent,
            tool_name,
        }
    }

    fn compile(
        &self,
        registrations: &[RegisteredToolMiddleware],
        universal: &[ToolMiddlewareInstallation],
        environment: &BTreeMap<ToolName, ToolBindingInput>,
        agent: &BTreeMap<AgentTypeName, BTreeMap<ToolName, ToolBindingInput>>,
        mode: ToolCompatibilityMode,
    ) -> super::CompiledToolMiddlewareChains {
        compile_tool_middleware_chains(
            DeploymentRevision::INITIAL,
            std::slice::from_ref(&self.tool),
            std::slice::from_ref(&self.binding),
            registrations,
            universal,
            environment,
            agent,
            mode,
        )
    }
}

#[test]
fn same_ref_id_in_distinct_graphs_does_not_hide_different_payloads() {
    let presented = tool(
        "presented",
        error(
            "failure",
            ErrorKind::RuntimeError,
            1,
            SchemaType::ref_to(TypeId::new("node")),
        ),
        recursive_graph("node", SchemaType::string()),
    );
    let next = tool(
        "next",
        error(
            "failure",
            ErrorKind::RuntimeError,
            1,
            SchemaType::ref_to(TypeId::new("node")),
        ),
        recursive_graph("node", SchemaType::u64()),
    );

    assert!(synthesize_effective_definition(&presented, None, &next).is_err());
}

#[test]
fn different_ref_ids_with_equivalent_recursive_schemas_are_accepted() {
    let presented_id = TypeId::new("presented-node");
    let next_id = TypeId::new("next-node");
    let presented = tool(
        "presented",
        error(
            "failure",
            ErrorKind::RuntimeError,
            7,
            SchemaType::ref_to(presented_id.clone()),
        ),
        recursive_graph(
            presented_id.as_str(),
            SchemaType::option(SchemaType::ref_to(presented_id.clone())),
        ),
    );
    let next = tool(
        "next",
        error(
            "failure",
            ErrorKind::RuntimeError,
            7,
            SchemaType::ref_to(next_id.clone()),
        ),
        recursive_graph(
            next_id.as_str(),
            SchemaType::option(SchemaType::ref_to(next_id.clone())),
        ),
    );

    synthesize_effective_definition(&presented, None, &next).unwrap();
}

#[test]
fn registry_is_validated_without_bindings() {
    let invalid = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree { nodes: Vec::new() },
        schema: SchemaGraph::empty(),
    };
    let registered = golem_common::model::tool::RegisteredTool {
        deployment_revision: DeploymentRevision::INITIAL,
        release_id: None,
        definition: invalid,
        provision: Default::default(),
        source: golem_common::model::tool::ToolSource::Component {
            component_id: Default::default(),
            component_revision: golem_common::model::component::ComponentRevision::INITIAL,
            component_name: "test:component".try_into().unwrap(),
        },
        owner_account_id: Default::default(),
        owner_account_email: "owner@example.com".into(),
        metadata_version: String::new(),
        metadata_digest: Default::default(),
    };

    let compiled = compile_tool_middleware_chains(
        DeploymentRevision::INITIAL,
        &[registered],
        &[],
        &[],
        &[],
        &Default::default(),
        &Default::default(),
        golem_common::schema::tool::compatibility::ToolCompatibilityMode::StructuralSubtype,
    );

    assert!(!compiled.errors.is_empty());
    assert!(
        compiled
            .errors
            .iter()
            .all(|error| { error.agent_type_name.is_none() && error.tool_name.is_none() })
    );
}

#[test]
fn per_tool_merge_preserves_omitted_empty_order_and_repetitions() {
    let binding = |middleware, mode| ToolBindingInput {
        middleware,
        middleware_merge_mode: Some(mode),
        ..Default::default()
    };
    let environment = binding(
        Some(vec![
            installation("environment", 1),
            installation("same", 2),
        ]),
        ToolMiddlewareMergeMode::Prepend,
    );
    let values = |items: Vec<ToolMiddlewareInstallation>| {
        items
            .into_iter()
            .map(|item| (item.name.to_string(), item.parameters.0["value"].as_i64()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        values(effective_installations(
            Some(&environment),
            Some(&binding(None, ToolMiddlewareMergeMode::Replace)),
        )),
        [("environment".into(), Some(1)), ("same".into(), Some(2))]
    );
    assert!(
        effective_installations(
            Some(&environment),
            Some(&binding(Some(Vec::new()), ToolMiddlewareMergeMode::Replace)),
        )
        .is_empty()
    );
    let agent = vec![installation("same", 3), installation("agent", 4)];
    assert_eq!(
        values(effective_installations(
            Some(&environment),
            Some(&binding(Some(agent), ToolMiddlewareMergeMode::Append)),
        )),
        [
            ("environment".into(), Some(1)),
            ("same".into(), Some(2)),
            ("same".into(), Some(3)),
            ("agent".into(), Some(4)),
        ]
    );
}

#[test]
fn compiler_builds_universal_and_monomorphic_chain_in_order_with_duplicate_parameters() {
    let fixture = CompilerFixture::new();
    let leaf = fixture.tool.definition.clone();
    let registrations = vec![
        registered_middleware("universal", ToolMiddlewareScope::Universal),
        registered_middleware(
            "scoped",
            ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
                presented: simple_tool("leaf"),
                expected: Some(leaf),
            })),
        ),
    ];
    let universal = vec![installation("universal", 10), installation("universal", 11)];
    let environment = BTreeMap::from([(
        fixture.tool_name.clone(),
        ToolBindingInput {
            middleware: Some(vec![installation("scoped", 20)]),
            ..Default::default()
        },
    )]);
    let agent = BTreeMap::from([(
        fixture.agent.clone(),
        BTreeMap::from([(
            fixture.tool_name.clone(),
            ToolBindingInput {
                middleware: Some(vec![installation("scoped", 21)]),
                middleware_merge_mode: Some(ToolMiddlewareMergeMode::Append),
                ..Default::default()
            },
        )]),
    )]);

    let compiled = fixture.compile(
        &registrations,
        &universal,
        &environment,
        &agent,
        ToolCompatibilityMode::StructuralSubtype,
    );
    assert!(compiled.errors.is_empty(), "{:?}", compiled.errors);
    let chain = &compiled.chains[0];
    assert_eq!(chain.effective_definition.name(), Some("leaf"));
    assert_eq!(
        chain
            .occurrences
            .iter()
            .map(|occurrence| (
                occurrence.middleware.definition.name.as_str(),
                occurrence.parameters.0["value"].as_i64(),
            ))
            .collect::<Vec<_>>(),
        [
            ("universal", Some(10)),
            ("universal", Some(11)),
            ("scoped", Some(20)),
            ("scoped", Some(21)),
        ]
    );
    assert!(
        chain.occurrences[..2]
            .iter()
            .all(|item| item.compatibility.is_none())
    );
    assert!(
        chain.occurrences[2..]
            .iter()
            .all(|item| item.compatibility.is_some())
    );
    assert!(chain.occurrences.iter().all(|item| {
        item.middleware.owner_account_email.as_str() == "middleware@example.com"
            && item.middleware.definition.version == "2.0.0"
            && item.middleware.metadata_version == "pinned-metadata"
            && matches!(
                item.middleware.source,
                ToolMiddlewareSource::Component { .. }
            )
    }));
}

#[test]
fn compiler_rejects_pin_scope_and_leaf_mismatches() {
    let fixture = CompilerFixture::new();
    let universal = registered_middleware("universal", ToolMiddlewareScope::Universal);
    let mut wrong_version = installation("universal", 1);
    wrong_version.version = Some("wrong".to_string());
    let mut wrong_account = installation("universal", 1);
    wrong_account.account = Some("wrong@example.com".into());
    for bad in [wrong_version, wrong_account] {
        let result = fixture.compile(
            std::slice::from_ref(&universal),
            &[bad],
            &BTreeMap::new(),
            &BTreeMap::new(),
            ToolCompatibilityMode::StructuralSubtype,
        );
        assert!(result.chains.is_empty());
        assert!(!result.errors.is_empty());
    }
    let wrong_scope = fixture.compile(
        std::slice::from_ref(&universal),
        &[],
        &BTreeMap::from([(
            fixture.tool_name.clone(),
            ToolBindingInput {
                middleware: Some(vec![installation("universal", 1)]),
                ..Default::default()
            },
        )]),
        &BTreeMap::new(),
        ToolCompatibilityMode::StructuralSubtype,
    );
    assert!(wrong_scope.chains.is_empty());
    assert!(
        wrong_scope
            .errors
            .iter()
            .any(|error| error.message.contains("wrong scope"))
    );

    let mut missing = fixture.binding.clone();
    missing.tool_name = ToolName::try_from("missing").unwrap();
    let result = compile_tool_middleware_chains(
        DeploymentRevision::INITIAL,
        std::slice::from_ref(&fixture.tool),
        &[missing],
        &[],
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        ToolCompatibilityMode::StructuralSubtype,
    );
    assert!(result.errors[0].message.contains("no registered leaf tool"));
}

#[test]
fn compiler_applies_structural_strict_and_nominal_compatibility_modes() {
    let fixture = CompilerFixture::new();
    for mode in [
        ToolCompatibilityMode::StructuralSubtype,
        ToolCompatibilityMode::StrictEquality,
        ToolCompatibilityMode::Nominal,
    ] {
        let registration = registered_middleware(
            "scoped",
            ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
                presented: simple_tool("binding"),
                expected: Some(fixture.tool.definition.clone()),
            })),
        );
        let result = fixture.compile(
            &[registration],
            &[],
            &BTreeMap::from([(
                fixture.tool_name.clone(),
                ToolBindingInput {
                    middleware: Some(vec![installation("scoped", 1)]),
                    ..Default::default()
                },
            )]),
            &BTreeMap::new(),
            mode,
        );
        assert!(result.errors.is_empty(), "{mode:?}: {:?}", result.errors);
        assert_eq!(
            result.chains[0].occurrences[0]
                .compatibility
                .as_ref()
                .unwrap()
                .mode,
            mode
        );
    }
}

#[test]
fn middleware_installation_owns_its_filesystem_policy() {
    let effective = effective_installations(
        Some(&ToolBindingInput {
            middleware: Some(vec![installation("audit", 1)]),
            ..Default::default()
        }),
        None,
    );
    assert_eq!(effective[0].filesystem_access, ToolFilesystemAccess::Denied);
}

#[test]
fn adapter_inherits_unknown_errors_at_every_presented_command() {
    let mut presented = simple_tool("outward");
    presented
        .commands
        .nodes
        .push(command("visible", Vec::new()));
    presented.commands.nodes[0].subcommands = vec![CommandIndex(1)];
    let expected = tool(
        "expected",
        error("known", ErrorKind::RuntimeError, 1, SchemaType::string()),
        SchemaGraph::empty(),
    );
    let mut next = simple_tool("inner");
    next.commands.nodes[0].body.as_mut().unwrap().errors =
        vec![empty_error("known"), empty_error("shared")];
    next.commands
        .nodes
        .push(command("unrelated", vec![empty_error("extra")]));
    next.commands.nodes[0].subcommands = vec![CommandIndex(1)];

    let synthesized = synthesize_effective_definition(&presented, Some(&expected), &next).unwrap();
    for node in &synthesized.commands.nodes {
        let names = node
            .body
            .as_ref()
            .unwrap()
            .errors
            .iter()
            .map(|case| case.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"extra"));
        assert!(names.contains(&"shared"));
    }
}

#[test]
fn inherited_error_payload_collision_is_rejected() {
    let presented = tool(
        "outward",
        error("same", ErrorKind::RuntimeError, 1, SchemaType::string()),
        SchemaGraph::empty(),
    );
    let next = tool(
        "inner",
        error("same", ErrorKind::RuntimeError, 1, SchemaType::u64()),
        SchemaGraph::empty(),
    );
    assert!(synthesize_effective_definition(&presented, None, &next).is_err());
}
