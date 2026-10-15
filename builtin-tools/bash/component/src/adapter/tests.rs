use super::*;
use golem_schema::tool::*;
use golem_schema::{
    IntoTypedSchemaValue, SchemaGraph, SchemaType, SchemaValue, try_into_schema_graph,
};

#[test]
fn successful_results_preserve_output_and_validate_exit_status() {
    for status in [0, 7, 255] {
        let typed = ProcessExitStatus { exit_code: status }
            .into_typed_schema_value()
            .unwrap();
        let wire = || golem_rust::encode_typed_schema_value(&typed).unwrap();
        let mut output = CommandOutput {
            stdout: b"payload".to_vec(),
            ..Default::default()
        };
        apply_result(&mut output, true, true, Some(wire()));
        assert_eq!(output.stdout, b"payload");
        assert_eq!(output.exit_code, status);
        assert!(output.stderr.is_empty());
        let mut output = CommandOutput::default();
        apply_result(&mut output, false, true, Some(wire()));
        assert_eq!(output.exit_code, status);
        assert!(output.stdout.is_empty());
        output.exit_code = 130;
        output.stderr = b"cancelled\n".to_vec();
        apply_result(&mut output, false, true, Some(wire()));
        assert_eq!(output.exit_code, 130);
        assert_eq!(output.stderr, b"cancelled\n");
    }
    for (type_, value, expected) in [
        (SchemaType::s32(), SchemaValue::S32(7), "7\n"),
        (
            SchemaType::string(),
            SchemaValue::String("text".into()),
            "text",
        ),
        (SchemaType::bool(), SchemaValue::Bool(true), "true\n"),
    ] {
        let wire = golem_rust::encode_typed_schema_value(&TypedSchemaValue::new(
            SchemaGraph::anonymous(type_),
            value,
        ))
        .unwrap();
        let mut output = CommandOutput::default();
        apply_result(&mut output, false, false, Some(wire));
        assert_eq!(output.stdout, expected.as_bytes());
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
    }
    let wire = golem_rust::encode_typed_schema_value(&TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::string()),
        SchemaValue::String("text".into()),
    ))
    .unwrap();
    let mut output = CommandOutput {
        stdout: b"stream".to_vec(),
        ..Default::default()
    };
    apply_result(&mut output, true, false, Some(wire));
    assert_eq!(output.stdout, b"stream");
    assert_eq!(output.exit_code, 0);
    let wire = golem_rust::encode_typed_schema_value(&TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::string()),
        SchemaValue::String("text".into()),
    ))
    .unwrap();
    apply_result(&mut output, true, true, Some(wire));
    assert_eq!(output.exit_code, 1);
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("expected named ProcessExitStatus")
    );
    let mut output = CommandOutput::default();
    apply_result(&mut output, true, true, None);
    assert_eq!(output.exit_code, 1);
    let mut wire = golem_rust::encode_typed_schema_value(
        &ProcessExitStatus { exit_code: 0 }
            .into_typed_schema_value()
            .unwrap(),
    )
    .unwrap();
    wire.value.root = i32::MAX;
    let mut output = CommandOutput {
        stdout: b"partial".to_vec(),
        stderr: b"provider\n".to_vec(),
        ..Default::default()
    };
    apply_result(&mut output, true, true, Some(wire));
    assert_eq!(output.exit_code, 1);
    assert_eq!(output.stdout, b"partial");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("provider\nresult:"));
    assert!(stderr.len() > "provider\nresult:\n".len());
}

fn fixture() -> Tool {
    Tool {
        version: "1.0.0".into(),
        requires_filesystem: false,
        schema: SchemaGraph::empty(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "fixture".into(),
                aliases: vec![],
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: vec![],
                body: Some(CommandBody {
                    positionals: Positionals {
                        fixed: vec![Positional {
                            name: "value".into(),
                            type_: SchemaType::string(),
                            doc: Doc::default(),
                            value_name: None,
                            default: None,
                            required: true,
                            accepts_stdio: false,
                        }],
                        ..Default::default()
                    },
                    options: vec![],
                    flags: vec![],
                    constraints: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    result: None,
                    errors: vec![],
                    annotations: None,
                }),
            }],
        },
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn named_process_status_drives_shell_status_and_conditionals() {
    struct Invoker(Catalog);
    struct Prepared(Invocation);
    impl CommandInvoker for Invoker {
        fn prepare(
            &self,
            name: &str,
            args: &[String],
            variables: &HashMap<String, String>,
        ) -> Result<Box<dyn PreparedCommand>, CommandOutput> {
            self.0
                .prepare_invocation(name, args, variables)
                .map(|call| Box::new(Prepared(call)) as Box<dyn PreparedCommand>)
        }
    }
    impl PreparedCommand for Prepared {
        fn takes_stdin(&self) -> bool {
            false
        }
        fn invoke(&self, _: Option<Vec<u8>>) -> CommandFuture<'_> {
            Box::pin(async move {
                let SchemaValue::Record { fields } = self.0.input.value() else {
                    panic!()
                };
                let SchemaValue::String(status) = &fields[0] else {
                    panic!()
                };
                let wire = golem_rust::encode_typed_schema_value(
                    &ProcessExitStatus {
                        exit_code: status.parse().unwrap(),
                    }
                    .into_typed_schema_value()
                    .unwrap(),
                )
                .unwrap();
                let mut output = CommandOutput {
                    stdout: b"out\n".to_vec(),
                    ..Default::default()
                };
                apply_result(
                    &mut output,
                    self.0.stdout,
                    self.0.result_is_status,
                    Some(wire),
                );
                output
            })
        }
    }
    let mut tool = fixture();
    tool.schema = try_into_schema_graph::<ProcessExitStatus>().unwrap();
    let body = tool.commands.nodes[0].body.as_mut().unwrap();
    body.stdout = Some(StreamSpec {
        doc: Doc::default(),
        mime: vec![],
        required: false,
    });
    body.result = Some(ResultSpec {
        type_: tool.schema.root.clone(),
        doc: Doc::default(),
        formatters: vec![Formatter {
            name: "json".into(),
            doc: Doc::default(),
        }],
        default_formatter: "json".into(),
    });
    let catalog = Catalog::new(BTreeMap::from([("probe".into(), tool)])).unwrap();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let mut shell = bash_shell::session::Session::new().await.unwrap();
        shell.register_commands(vec![bash_shell::commands::CommandDescriptor {
            name: "probe".into(), help: String::new(),
        }], std::sync::Arc::new(Invoker(catalog))).unwrap();
        let output = shell.run("probe 7").await;
        assert_eq!(output.exit_code, 7);
        assert_eq!(output.stdout, b"out\n");
        assert!(output.stderr.is_empty());
        let output = shell.run("probe 7; echo $?; probe 7 && echo wrong || echo failed; probe 0 && echo passed || echo wrong").await;
        assert_eq!(output.stdout, b"out\n7\nout\nfailed\nout\npassed\n");
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
    });
}

#[test]
fn projection_uses_the_cli_parser_and_finishes_help_before_input() {
    let status_graph = try_into_schema_graph::<ProcessExitStatus>().unwrap();
    let anonymous = SchemaGraph::anonymous(status_graph.defs[0].body.clone());
    let mut lookalike = status_graph.clone();
    lookalike.defs[0].id = "example.Status".into();
    lookalike.root = SchemaType::ref_to("example.Status".into());
    for (graph, expected_status) in [
        (status_graph.clone(), true),
        (anonymous, false),
        (lookalike, false),
        (SchemaGraph::anonymous(SchemaType::s32()), false),
    ] {
        let mut tool = fixture();
        tool.commands.nodes[0].body.as_mut().unwrap().result = Some(ResultSpec {
            type_: graph.root.clone(),
            doc: Doc::default(),
            formatters: vec![Formatter {
                name: "json".into(),
                doc: Doc::default(),
            }],
            default_formatter: "json".into(),
        });
        tool.schema = graph;
        let catalog = Catalog::new(BTreeMap::from([("probe".into(), tool)])).unwrap();
        let call = catalog
            .prepare_invocation("probe", &["hello".into()], &HashMap::new())
            .unwrap_or_else(|output| panic!("{:?}", output.stderr));
        assert_eq!(call.result_is_status, expected_status);
    }
    let mut malformed = fixture();
    malformed.schema = status_graph;
    let SchemaType::Record { fields, .. } = &mut malformed.schema.defs[0].body else {
        panic!()
    };
    fields[0].body = SchemaType::s32();
    malformed.commands.nodes[0].body.as_mut().unwrap().result = Some(ResultSpec {
        type_: malformed.schema.root.clone(),
        doc: Doc::default(),
        formatters: vec![Formatter {
            name: "json".into(),
            doc: Doc::default(),
        }],
        default_formatter: "json".into(),
    });
    assert!(Catalog::new(BTreeMap::from([("probe".into(), malformed)])).is_err());
    let tool = fixture();
    let catalog = Catalog::new(BTreeMap::from([("renamed".into(), tool.clone())])).unwrap();
    let args = vec!["hello with spaces".into()];
    let ParsedToolArguments::Invoke {
        input: direct,
        command_path,
    } = argv::parse(&tool, &args).unwrap()
    else {
        panic!("expected invocation")
    };
    let prepared = catalog
        .prepare("renamed", &args, &HashMap::new())
        .unwrap_or_else(|output| panic!("{:?}", output.stderr));
    assert!(!prepared.takes_stdin());
    let projected = catalog
        .prepare_invocation("renamed", &args, &HashMap::new())
        .unwrap_or_else(|output| panic!("{:?}", output.stderr));
    assert_eq!(projected.input, *direct);
    assert_eq!(projected.path, command_path);
    assert_eq!(projected.name, "renamed");
    assert!(command_path.is_empty());
    assert_eq!(
        direct.value(),
        &golem_schema::SchemaValue::Record {
            fields: vec![golem_schema::SchemaValue::String(
                "hello with spaces".into()
            )],
        }
    );
    let expected_help = match argv::parse(&tool, &["--help".into()]).unwrap() {
        ParsedToolArguments::Help(text) => text,
        _ => panic!("expected help"),
    };
    let output = match catalog.prepare("renamed", &["--help".into()], &HashMap::new()) {
        Err(output) => output,
        Ok(_) => panic!("help must not invoke or consume input"),
    };
    assert_eq!(output.stdout, expected_help.as_bytes());
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    for args in [vec![], vec!["--invalid".into()]] {
        let expected = argv::parse(&tool, &args).err().unwrap();
        let output = catalog
            .prepare("renamed", &args, &HashMap::new())
            .err()
            .unwrap();
        assert_eq!(output.stderr, format!("{expected}\n").as_bytes());
        assert_eq!(output.exit_code, 2);
    }
}

#[test]
fn tool_failures_map_to_shell_statuses_and_messages() {
    use golem_rust::schema::wit::wire::{CustomToolError, ToolError, ToolRpcError};
    let declared: BTreeMap<String, u8> = [("quota".to_string(), 42)].into();
    let custom = |name: &str| {
        let payload = golem_rust::encode_typed_schema_value(
            &golem_schema::try_into_typed_schema_value(&"over".to_string()).unwrap(),
        )
        .unwrap();
        ToolRpcError::RemoteToolError(ToolError::CustomError(CustomToolError {
            name: name.into(),
            payload,
        }))
    };
    let cases = [
        (ToolRpcError::Denied("no grant".into()), 3, "no grant"),
        (ToolRpcError::NotFound("gone".into()), 127, "gone"),
        (ToolRpcError::Cancelled, 130, "Cancelled"),
        (
            ToolRpcError::RemoteToolError(ToolError::InvalidInput("bad".into())),
            2,
            "bad",
        ),
        (
            ToolRpcError::RemoteToolError(ToolError::ConstraintViolation("too big".into())),
            2,
            "too big",
        ),
        (ToolRpcError::ProtocolError("broken".into()), 1, "broken"),
        (ToolRpcError::ResourceExhausted("limit".into()), 1, "limit"),
        (custom("quota"), 42, "tool error: quota: \"over\""),
        // A name the tool did not declare still fails, with the generic status.
        (custom("surprise"), 1, "tool error: surprise: \"over\""),
    ];
    for (error, code, message) in cases {
        assert_eq!(rpc_failure(error, &declared), (code, message.to_string()));
    }
}

fn option(name: &str, shape: OptionShape, default: Option<SchemaValue>) -> OptionSpec {
    OptionSpec {
        long: name.into(),
        short: None,
        aliases: vec![],
        doc: Doc::default(),
        value_name: None,
        shape,
        default,
        required: false,
        env_var: None,
    }
}

/// A tool with one command, `run`, that takes `options`.
fn tool_with(options: Vec<OptionSpec>) -> Tool {
    let mut tool = fixture();
    let mut run = tool.commands.nodes[0].clone();
    run.name = "run".into();
    let body = run.body.as_mut().unwrap();
    body.positionals = Positionals::default();
    body.options = options;
    tool.commands.nodes[0].body = None;
    tool.commands.nodes[0].subcommands = vec![CommandIndex(1)];
    tool.commands.nodes.push(run);
    tool
}

fn string(value: &str) -> SchemaValue {
    SchemaValue::String(value.into())
}

/// The input record that `tool run ARGS` gives when the shell has `variables`.
fn record(tool: Tool, args: &[&str], variables: &[(&str, &str)]) -> Vec<SchemaValue> {
    let catalog = Catalog::new(BTreeMap::from([("tool".into(), tool)])).unwrap();
    let args: Vec<String> = ["run"].iter().chain(args).map(|s| s.to_string()).collect();
    let variables: HashMap<String, String> = variables
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let invocation = catalog
        .prepare_invocation("tool", &args, &variables)
        .unwrap_or_else(|output| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    match invocation.input.value() {
        SchemaValue::Record { fields } => fields.clone(),
        other => panic!("not a record: {other:?}"),
    }
}

#[test]
fn a_declared_variable_reaches_the_input_record() {
    let tool = || {
        let mut region = option(
            "region",
            OptionShape::Scalar(SchemaType::string()),
            Some(string("local")),
        );
        region.env_var = Some("REGION".into());
        tool_with(vec![region])
    };
    assert_eq!(record(tool(), &[], &[]), vec![string("local")]);
    assert_eq!(record(tool(), &[], &[("REGION", "eu")]), vec![string("eu")]);
    assert_eq!(
        record(tool(), &["--region", "us"], &[("REGION", "eu")]),
        vec![string("us")]
    );
}

#[test]
fn an_option_gets_the_shell_directory_only_when_it_declares_the_variable() {
    // The shape of `npm`, `node` and `tsc`: a plain `--cwd` with a default and no declaration.
    let plain = || {
        tool_with(vec![option(
            "cwd",
            OptionShape::Scalar(SchemaType::string()),
            Some(string("/workspace")),
        )])
    };
    assert_eq!(
        record(plain(), &[], &[("PWD", "/work/app")]),
        vec![string("/workspace")]
    );
    assert_eq!(
        record(plain(), &["--cwd", "/x"], &[("PWD", "/work/app")]),
        vec![string("/x")]
    );
    // The shape of `git`: an inherited global `--cwd` that declares `PWD`.
    let declared = || {
        let mut cwd = option(
            "cwd",
            OptionShape::Scalar(SchemaType::string()),
            Some(string("/")),
        );
        cwd.env_var = Some("PWD".into());
        let mut tool = tool_with(vec![]);
        tool.commands.nodes[0].globals.options = vec![cwd];
        tool
    };
    assert_eq!(record(declared(), &[], &[]), vec![string("/")]);
    assert_eq!(
        record(declared(), &[], &[("PWD", "/work/repo")]),
        vec![string("/work/repo")]
    );
}
