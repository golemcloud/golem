use super::*;
use golem_schema::tool::*;
use golem_schema::{SchemaGraph, SchemaType};

fn fixture() -> Tool {
    Tool {
        version: "1.0.0".into(),
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
                    result: None,
                    errors: vec![],
                    annotations: None,
                }),
            }],
        },
    }
}

#[test]
fn projection_uses_the_cli_parser_and_finishes_help_before_input() {
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
        .prepare("renamed", &args)
        .unwrap_or_else(|output| panic!("{:?}", output.stderr));
    assert!(!prepared.takes_stdin());
    let projected = catalog
        .prepare_invocation("renamed", &args)
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
    let output = match catalog.prepare("renamed", &["--help".into()]) {
        Err(output) => output,
        Ok(_) => panic!("help must not invoke or consume input"),
    };
    assert_eq!(output.stdout, expected_help.as_bytes());
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    for args in [vec![], vec!["--invalid".into()]] {
        let expected = argv::parse(&tool, &args).err().unwrap();
        let output = catalog.prepare("renamed", &args).err().unwrap();
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
