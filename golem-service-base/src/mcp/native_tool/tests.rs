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
    )
}

fn tool(root: CommandNode) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
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
    assert_eq!(public["maybe"], json!({"$option": "none"}));
    assert_eq!(public["defaulted"], "kept");

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
    )
    .unwrap();
    assert_eq!(exports.len(), 8);
    assert!(
        exports
            .iter()
            .all(|export| export.command_path == ["do-work", "deep"])
    );
    let excluded = compile_native_tool_exports(
        ComponentId::new(),
        "test:owner".try_into().unwrap(),
        "root".try_into().unwrap(),
        &definition,
        None,
        Some(&["do-work **".to_string()]),
    )
    .unwrap();
    assert_eq!(
        excluded
            .iter()
            .map(|e| e.mcp_name.as_str())
            .collect::<Vec<_>>(),
        ["root", "r"]
    );

    let mut collision_root = node("root", None);
    collision_root.subcommands = vec![CommandIndex(1), CommandIndex(2)];
    let mut parent = node("a", None);
    parent.subcommands.push(CommandIndex(3));
    let collision = Tool {
        version: "1.0.0".to_string(),
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
    assert!(compile(&collision).unwrap_err().contains("collision"));
}

#[test]
fn referenced_record_output_is_wrapped_in_an_mcp_root_object() {
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
    assert!(output["properties"][FALLBACK_OUTPUT_FIELD_NAME]["$ref"].is_string());
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
            None
        )
        .unwrap_err()
        .contains("no executable commands")
    );
}
