// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use golem_common::schema::metadata::TypeId;
use golem_common::schema::public_json::encode_public_schema_value;
use golem_common::schema::tool::*;
use golem_common::schema::{MetadataEnvelope, NamedFieldType, SchemaTypeDef};
use test_r::test;

fn body() -> CommandBody {
    CommandBody {
        positionals: Positionals::default(),
        options: Vec::new(),
        flags: Vec::new(),
        constraints: Vec::new(),
        stdin: None,
        stdout: None,
        stderr: None,
        result: None,
        errors: Vec::new(),
        annotations: None,
    }
}

fn node(name: &str, body: Option<CommandBody>) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        aliases: Vec::new(),
        doc: Doc::default(),
        http: None,
        globals: Globals::default(),
        subcommands: Vec::new(),
        body,
    }
}

fn option(long: &str, shape: OptionShape, required: bool) -> OptionSpec {
    OptionSpec {
        long: long.to_string(),
        short: None,
        aliases: Vec::new(),
        doc: Doc::default(),
        value_name: None,
        shape,
        default: None,
        required,
        env_var: None,
    }
}

fn compile(tool: &Tool) -> Result<Vec<CompiledMcpToolExport>, String> {
    compile_native_tool_exports(
        ComponentId::new(),
        "test:owner".try_into().unwrap(),
        tool.commands.nodes[0].name.as_str().try_into().unwrap(),
        tool,
        None,
        None,
        None,
    )
}

fn tool(root: CommandNode) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        commands: CommandTree { nodes: vec![root] },
        schema: SchemaGraph::empty(),
    }
}

#[test]
fn omission_respects_author_types_defaults_and_collection_bounds() {
    let mut command = body();
    command.options = vec![
        option(
            "plain",
            OptionShape::OptionalScalar(SchemaType::string()),
            false,
        ),
        option(
            "maybe",
            OptionShape::Scalar(SchemaType::option(SchemaType::string())),
            false,
        ),
        option(
            "many",
            OptionShape::RepeatableList(RepeatableListShape {
                repetition: Repetition::Repeated,
                item_type: SchemaType::string(),
            }),
            true,
        ),
    ];
    command.options.push(OptionSpec {
        default: Some(SchemaValue::String("kept".to_string())),
        ..option("defaulted", OptionShape::Scalar(SchemaType::string()), true)
    });
    command.positionals.tail = Some(TailPositional {
        name: "tail".to_string(),
        doc: Doc::default(),
        value_name: None,
        item_type: SchemaType::string(),
        min: 1,
        max: Some(2),
        separator: None,
        verbatim: false,
        accepts_stdio: false,
    });
    let export = compile(&tool(node("root", Some(command))))
        .unwrap()
        .remove(0);
    let schema = export.input_json_schema().unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("plain")));
    assert!(!required.contains(&json!("maybe")));
    assert!(required.contains(&json!("many")));
    assert!(required.contains(&json!("tail")));
    assert!(!required.contains(&json!("defaulted")));
    assert_eq!(schema["properties"]["many"]["minItems"], 1);
    assert_eq!(schema["properties"]["tail"]["minItems"], 1);
    assert_eq!(schema["properties"]["tail"]["maxItems"], 2);

    let error = export.parse_arguments(Map::new()).unwrap_err();
    assert!(error.contains("tail"), "{error}");
    let mut args = Map::new();
    args.insert("plain".to_string(), json!("x"));
    args.insert("many".to_string(), json!(["x"]));
    args.insert("tail".to_string(), json!(["x"]));
    let (value, _) = export.parse_arguments(args).unwrap();
    let public = encode_public_schema_value(
        value.graph(),
        value.root_type(),
        value.value(),
        |_, _| unreachable!(),
    )
    .unwrap();
    assert_eq!(
        public["value"]["fields"][2],
        json!({"kind": "option", "value": {"inner": null}})
    );
    assert_eq!(
        public["value"]["fields"][4],
        json!({"kind": "string", "value": "kept"})
    );

    let mut excessive = Map::new();
    excessive.insert("plain".to_string(), json!("x"));
    excessive.insert("many".to_string(), json!(["x"]));
    excessive.insert("tail".to_string(), json!(["x", "y", "z"]));
    assert!(
        export
            .parse_arguments(excessive)
            .unwrap_err()
            .contains("at most 2")
    );
}

#[test]
fn root_aliases_paths_globs_and_normalized_collisions_are_bounded() {
    let mut root = node("root", Some(body()));
    root.aliases.push("r".to_string());
    let mut child = node("do-work", Some(body()));
    child.aliases.push("run".to_string());
    let mut grandchild = node("deep", Some(body()));
    child.subcommands.push(CommandIndex(2));
    root.subcommands.push(CommandIndex(1));
    grandchild.aliases.push("d".to_string());
    let definition = Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        commands: CommandTree {
            nodes: vec![root, child, grandchild],
        },
        schema: SchemaGraph::empty(),
    };
    let include = vec!["** deep".to_string()];
    let exports = compile_native_tool_exports(
        ComponentId::new(),
        "test:owner".try_into().unwrap(),
        "root".try_into().unwrap(),
        &definition,
        Some(&include),
        None,
        None,
    )
    .unwrap();
    assert_eq!(exports.len(), 8);
    assert!(
        exports
            .iter()
            .all(|export| export.command_path == ["do-work", "deep"])
    );
    let description = |mcp_name: &str| {
        exports
            .iter()
            .find(|export| export.mcp_name == mcp_name)
            .map(|export| export.description.as_str())
            .unwrap()
    };
    assert!(
        !description("root_do_work_deep").contains("(alias for"),
        "{}",
        description("root_do_work_deep")
    );
    assert!(
        description("r_do_work_deep").ends_with(" (alias for root do-work deep)"),
        "{}",
        description("r_do_work_deep")
    );
    let excluded = compile_native_tool_exports(
        ComponentId::new(),
        "test:owner".try_into().unwrap(),
        "root".try_into().unwrap(),
        &definition,
        None,
        Some(&["do-work **".to_string()]),
        None,
    )
    .unwrap();
    assert_eq!(
        excluded
            .iter()
            .map(|e| e.mcp_name.as_str())
            .collect::<Vec<_>>(),
        ["root", "r"]
    );
    assert_eq!(
        excluded
            .iter()
            .map(CompiledMcpToolExport::command_label)
            .collect::<Vec<_>>(),
        ["tool command `root`", "tool command `r` of tool `root`"]
    );

    let mut collision_root = node("root", None);
    collision_root.subcommands = vec![CommandIndex(1), CommandIndex(2)];
    let mut parent = node("a", None);
    parent.subcommands.push(CommandIndex(3));
    let collision = Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        commands: CommandTree {
            nodes: vec![
                collision_root,
                node("a-b", Some(body())),
                parent,
                node("b", Some(body())),
            ],
        },
        schema: SchemaGraph::empty(),
    };
    let error = compile(&collision).unwrap_err();
    assert!(error.contains("`root a-b`"), "{error}");
    assert!(error.contains("`root a b`"), "{error}");
    assert!(error.contains("`root_a_b`"), "{error}");
}

#[test]
fn an_alias_path_that_collides_with_another_command_is_named_by_its_alias_path() {
    let mut root = node("root", None);
    root.subcommands = vec![CommandIndex(1), CommandIndex(2)];
    let mut parent = node("x", None);
    parent.aliases.push("a".to_string());
    parent.subcommands.push(CommandIndex(3));
    let definition = Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        commands: CommandTree {
            nodes: vec![
                root,
                node("a-b", Some(body())),
                parent,
                node("b", Some(body())),
            ],
        },
        schema: SchemaGraph::empty(),
    };
    assert_eq!(
        compile(&definition).unwrap_err(),
        "tool command `root a-b` and tool command `root a b` both project to the MCP tool name `root_a_b`"
    );
}

#[test]
fn names_project_to_snake_case() {
    ["searchItems", "search-items", "search_items", "SearchItems"]
        .into_iter()
        .for_each(|name| assert_eq!(snake_case(name), "search_items", "{name}"));
    assert_eq!(snake_case("HTTPServer"), "http_server");
    assert_eq!(snake_case("parseURL"), "parse_url");
    assert_eq!(snake_case("item2Go"), "item2_go");
    assert_eq!(snake_case("v2"), "v2");
    assert_eq!(
        projected_tool_name("my-tool", &["searchItems".to_string(), "by-id".to_string()]),
        "my_tool_search_items_by_id"
    );
}

#[test]
fn arguments_project_to_snake_case_in_the_schema_and_the_call() {
    let mut command = body();
    command.options = vec![option(
        "dry-run",
        OptionShape::Scalar(SchemaType::string()),
        true,
    )];
    let export = compile(&tool(node("root", Some(command))))
        .unwrap()
        .remove(0);
    let schema = export.input_json_schema().unwrap();
    assert_eq!(schema["required"], json!(["dry_run"]));
    assert!(schema["properties"]["dry_run"].is_object(), "{schema:?}");
    assert!(schema["properties"].get("dry-run").is_none(), "{schema:?}");
    let (value, _) = export
        .parse_arguments(Map::from_iter([("dry_run".to_string(), json!("x"))]))
        .unwrap();
    assert_eq!(
        value.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::String("x".to_string())]
        }
    );
    assert!(
        export
            .parse_arguments(Map::from_iter([("dry-run".to_string(), json!("x"))]))
            .unwrap_err()
            .contains("unknown argument `dry-run`")
    );
}

#[test]
fn each_reserved_argument_is_rejected_with_the_command_and_the_argument() {
    [
        "_stdin",
        "idempotency-key",
        "idempotencyKey",
        "_agent",
        "_agent_id",
    ]
    .into_iter()
    .for_each(|argument| {
        let mut command = body();
        command.options = vec![option(
            argument,
            OptionShape::Scalar(SchemaType::string()),
            true,
        )];
        let mut child = node("search", Some(command));
        child.aliases.clear();
        let mut root = node("root", None);
        root.subcommands.push(CommandIndex(1));
        let definition = Tool {
            commands: CommandTree {
                nodes: vec![root, child],
            },
            ..tool(node("root", None))
        };
        let error =
            check_reserved_arguments(&definition, 1, "tool command `root search`").unwrap_err();
        assert!(error.contains("`root search`"), "{argument}: {error}");
        assert!(
            error.contains(&format!("`{argument}`")),
            "{argument}: {error}"
        );
    });
    let mut command = body();
    command.options = vec![option(
        "idempotency-keys",
        OptionShape::Scalar(SchemaType::string()),
        true,
    )];
    assert!(check_reserved_arguments(&tool(node("root", Some(command))), 0, "root").is_ok());
}

#[test]
fn the_reserved_argument_check_covers_only_the_selected_commands() {
    let mut keyed = body();
    keyed.options = vec![option(
        "idempotency-key",
        OptionShape::Scalar(SchemaType::string()),
        false,
    )];
    let mut root = node("root", None);
    root.subcommands = vec![CommandIndex(1), CommandIndex(2)];
    let definition = Tool {
        commands: CommandTree {
            nodes: vec![
                root,
                node("keyed", Some(keyed)),
                node("plain", Some(body())),
            ],
        },
        ..tool(node("root", None))
    };
    let error = compile(&definition).unwrap_err();
    assert!(error.contains("`root keyed`"), "{error}");
    assert!(error.contains("`idempotency-key`"), "{error}");
    assert!(error.contains("`idempotency_key`"), "{error}");
    let compile_with = |include: Option<&[String]>, exclude: Option<&[String]>| {
        compile_native_tool_exports(
            ComponentId::new(),
            "test:owner".try_into().unwrap(),
            "root".try_into().unwrap(),
            &definition,
            include,
            exclude,
            None,
        )
    };
    let selected = |exports: Vec<CompiledMcpToolExport>| {
        exports
            .into_iter()
            .map(|export| export.mcp_name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        selected(compile_with(None, Some(&["keyed".to_string()])).unwrap()),
        ["root_plain"]
    );
    assert_eq!(
        selected(compile_with(Some(&["plain".to_string()]), None).unwrap()),
        ["root_plain"]
    );
}

#[test]
fn mcp_name_replaces_the_tool_name_and_its_aliases_in_every_command() {
    let mut root = node("root", Some(body()));
    root.aliases.push("r".to_string());
    root.subcommands.push(CommandIndex(1));
    let mut child = node("do-work", Some(body()));
    child.aliases.push("run".to_string());
    let definition = Tool {
        commands: CommandTree {
            nodes: vec![root, child],
        },
        ..tool(node("root", None))
    };
    let exports = compile_native_tool_exports(
        ComponentId::new(),
        "test:owner".try_into().unwrap(),
        "root".try_into().unwrap(),
        &definition,
        None,
        None,
        Some("fileSearch"),
    )
    .unwrap();
    assert_eq!(
        exports
            .iter()
            .map(|export| export.mcp_name.as_str())
            .collect::<Vec<_>>(),
        ["file_search", "file_search_do_work", "file_search_run"]
    );
    assert_eq!(
        exports[2].command_label(),
        "tool command `fileSearch run` of tool `root`"
    );
    assert_eq!(
        exports
            .iter()
            .map(|export| export.description.as_str())
            .collect::<Vec<_>>(),
        ["", "", " (alias for do-work)"]
    );
    assert!(
        exports
            .iter()
            .all(|export| export.tool_name.as_str() == "root")
    );
}

#[test]
fn an_mcp_name_without_a_letter_or_digit_is_rejected() {
    let definition = tool(node("root", Some(body())));
    ["", "-", "_", "--"].into_iter().for_each(|name_override| {
        let error = compile_native_tool_exports(
            ComponentId::new(),
            "test:owner".try_into().unwrap(),
            "root".try_into().unwrap(),
            &definition,
            None,
            None,
            Some(name_override),
        )
        .unwrap_err();
        assert!(
            error.contains(&format!("`{name_override}`")),
            "{name_override:?}: {error}"
        );
    });
}

#[test]
fn the_constraint_text_uses_the_projected_argument_names() {
    let mut command = body();
    command.options = vec![
        option(
            "dry-run",
            OptionShape::OptionalScalar(SchemaType::string()),
            false,
        ),
        OptionSpec {
            aliases: vec!["all".to_string()],
            ..option(
                "force-all",
                OptionShape::OptionalScalar(SchemaType::string()),
                false,
            )
        },
        option(
            "keep-going",
            OptionShape::OptionalScalar(SchemaType::string()),
            false,
        ),
    ];
    command.constraints = vec![
        Constraint::AllOrNone(vec![
            Ref::Present("dry-run".to_string()),
            Ref::Present("force-all".to_string()),
        ]),
        Constraint::Implies(ImpliesC {
            lhs_quant: Quantifier::All,
            lhs: vec![Ref::Present("dry-run".to_string())],
            rhs_quant: Quantifier::Any,
            rhs: vec![Ref::Present("force-all".to_string())],
        }),
        Constraint::Forbids(ForbidsC {
            lhs_quant: Quantifier::All,
            lhs: vec![Ref::Present("all".to_string())],
            rhs: vec![Ref::Present("keep-going".to_string())],
        }),
    ];
    let export = compile(&tool(node("root", Some(command))))
        .unwrap()
        .remove(0);
    let description = export.description;
    assert!(description.contains("\"dry_run\""), "{description}");
    assert!(description.contains("\"force_all\""), "{description}");
    assert!(!description.contains("dry-run"), "{description}");
    assert!(!description.contains("force-all"), "{description}");
    let present = |name: &str| Ref::Present(name.to_string());
    let expected = serde_json::to_string(&[
        Constraint::AllOrNone(vec![present("dry_run"), present("force_all")]),
        Constraint::Implies(ImpliesC {
            lhs_quant: Quantifier::All,
            lhs: vec![present("dry_run")],
            rhs_quant: Quantifier::Any,
            rhs: vec![present("force_all")],
        }),
        Constraint::Forbids(ForbidsC {
            lhs_quant: Quantifier::All,
            lhs: vec![present("force_all")],
            rhs: vec![present("keep_going")],
        }),
    ])
    .unwrap();
    assert!(
        description.contains(&format!(" Constraints: {expected}.")),
        "{description}"
    );
}

#[test]
fn referenced_record_output_exposes_its_fields_at_the_mcp_root() {
    let id = TypeId::new("answer");
    let mut command = body();
    command.result = Some(ResultSpec {
        type_: SchemaType::ref_to(id.clone()),
        doc: Doc::default(),
        formatters: vec![Formatter {
            name: "json".to_string(),
            doc: Doc::default(),
        }],
        default_formatter: "json".to_string(),
    });
    let mut definition = tool(node("root", Some(command)));
    definition.schema.defs.push(SchemaTypeDef {
        id,
        name: Some("Answer".to_string()),
        body: SchemaType::record(vec![NamedFieldType {
            name: "value".to_string(),
            body: SchemaType::string(),
            metadata: MetadataEnvelope::default(),
        }]),
    });
    let output = compile(&definition).unwrap()[0]
        .output_json_schema()
        .unwrap()
        .unwrap();
    assert_eq!(output["type"], "object");
    assert_eq!(output["properties"]["value"]["type"], "string");
    assert!(!output.contains_key("$ref"));
    assert!(output["$defs"].is_object());
}

#[test]
fn capabilities_in_custom_errors_are_rejected() {
    let mut command = body();
    command.errors.push(ErrorCase {
        name: "denied".to_string(),
        doc: Doc::default(),
        kind: ErrorKind::RuntimeError,
        exit_code: 1,
        payload: Some(SchemaType::secret(Default::default())),
    });
    let error = compile(&tool(node("root", Some(command)))).unwrap_err();
    assert!(error.contains("custom error `denied`"), "{error}");
    assert!(error.contains("secret"), "{error}");
}

#[test]
fn mcp_options_use_human_json_and_required_matches_omission() {
    let mut command = body();
    command.options = vec![option(
        "maybe",
        OptionShape::Scalar(SchemaType::option(SchemaType::string())),
        true,
    )];
    let export = compile(&tool(node("root", Some(command))))
        .unwrap()
        .remove(0);
    assert_eq!(
        export.input_json_schema().unwrap()["required"],
        json!(["maybe"])
    );
    assert!(export.parse_arguments(Map::new()).is_err());
    for (json, expected) in [
        (Value::Null, SchemaValue::Option { inner: None }),
        (
            json!("hello"),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("hello".to_string()))),
            },
        ),
    ] {
        let (actual, _) = export
            .parse_arguments(Map::from_iter([("maybe".to_string(), json)]))
            .unwrap();
        assert_eq!(
            actual.value(),
            &SchemaValue::Record {
                fields: vec![expected]
            }
        );
    }
    assert!(
        export
            .parse_arguments(Map::from_iter([(
                "maybe".to_string(),
                json!({"$option":"none"})
            )]))
            .is_err()
    );
    assert!(
        export
            .parse_arguments(Map::from_iter([("unknown".to_string(), Value::Null)]))
            .unwrap_err()
            .contains("unknown argument")
    );
}

#[test]
fn compiled_root_export_round_trips_through_protobuf() {
    let export = compile(&tool(node("root", Some(body()))))
        .unwrap()
        .remove(0);
    let encoded: golem_api_grpc::proto::golem::mcp::CompiledMcpToolExport = export.clone().into();
    let decoded = CompiledMcpToolExport::try_from(encoded).unwrap();
    assert_eq!(decoded, export);
    assert!(decoded.command_path.is_empty());
}

#[test]
fn finite_stdin_requires_declared_role_and_enforces_decoded_byte_limit() {
    let mut command = body();
    command.stdin = Some(StreamSpec {
        doc: Doc::default(),
        mime: vec!["application/octet-stream".to_string()],
        required: true,
    });
    let export = compile(&tool(node("root", Some(command))))
        .unwrap()
        .remove(0);
    assert!(
        export
            .parse_arguments(Map::new())
            .unwrap_err()
            .contains("_stdin")
    );
    let args = |value: String| Map::from_iter([("_stdin".to_string(), Value::String(value))]);
    assert_eq!(
        export.parse_arguments(args("AP8R".into())).unwrap().1,
        Some(vec![0, 255, 17])
    );
    assert!(export.parse_arguments(args("not base64!".into())).is_err());
    assert_eq!(
        export
            .parse_arguments(args(STANDARD.encode(vec![7; MAX_STDIN])))
            .unwrap()
            .1
            .unwrap()
            .len(),
        MAX_STDIN
    );
    assert!(
        export
            .parse_arguments(args(STANDARD.encode(vec![7; MAX_STDIN + 1])))
            .unwrap_err()
            .contains("16 MiB")
    );
    let undeclared = compile(&tool(node("root", Some(body()))))
        .unwrap()
        .remove(0);
    assert!(
        undeclared
            .parse_arguments(args("".into()))
            .unwrap_err()
            .contains("unknown argument")
    );
}

#[test]
fn nested_streams_and_filters_selecting_no_commands_fail_compilation() {
    let mut command = body();
    command.options.push(option(
        "nested",
        OptionShape::Scalar(SchemaType::option(SchemaType::stream(Some(
            SchemaType::u8(),
        )))),
        false,
    ));
    assert!(
        compile(&tool(node("root", Some(command))))
            .unwrap_err()
            .contains("typed streams")
    );
    let definition = tool(node("root", Some(body())));
    assert!(
        compile_native_tool_exports(
            ComponentId::new(),
            "test:owner".try_into().unwrap(),
            "root".try_into().unwrap(),
            &definition,
            Some(&["missing".to_string()]),
            None,
            None
        )
        .unwrap_err()
        .contains("no executable commands")
    );
}
