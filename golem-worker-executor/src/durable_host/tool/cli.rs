// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

//! CLI projection only. Discovery and invocation retain their existing durable
//! operations, caller binding rules, permission checks and filesystem lane.

use super::*;
use crate::preview2::golem::tool::host::{
    CommandInput, CommandOutput, CommandPreparation, HostPreparedCommand,
};
use golem_common::schema::tool::argv::{ParsedToolArguments, parse_with_variables};
use golem_schema::tool::process::{
    ProcessResult, process_status_type, render_custom_error, render_result,
};
use std::collections::{BTreeMap, HashMap};

pub struct PreparedCommandEntry {
    path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: bool,
    policy: ResultPolicy,
}

#[derive(Clone)]
struct ResultPolicy {
    stdout: bool,
    stderr: bool,
    result_is_status: bool,
    errors: BTreeMap<String, u8>,
}

enum Prepared {
    Output(CommandOutput),
    Invoke(Box<PreparedCommandEntry>),
}

fn output(exit_code: u8, message: impl std::fmt::Display) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: vec![],
        stderr: format!("{message}\n").into_bytes(),
    }
}

fn render_success(
    policy: &ResultPolicy,
    value: Option<TypedSchemaValue>,
    mut decode: impl FnMut(
        golem_common::schema::wit::wire::SchemaValueTree,
    ) -> Result<SchemaValue, String>,
) -> Result<ProcessResult, String> {
    let typed = value
        .map(|value| {
            // An owned value has already transferred its capabilities into the
            // host table. Consume them even when stdout makes its text unnecessary,
            // and before a malformed graph can reject the projection.
            let decoded = decode(value.value)?;
            let graph = decode_graph(&value.graph)
                .map_err(|_| "result: invalid schema graph".to_string())?;
            Ok::<_, String>(ModelTypedSchemaValue::new(graph, decoded))
        })
        .transpose()?;
    render_result(policy.stdout, policy.result_is_status, typed.as_ref())
}

fn prepare(tool: &Tool, args: &[String], variables: &HashMap<String, String>) -> Prepared {
    let parsed = match parse_with_variables(tool, args, variables) {
        Ok(parsed) => parsed,
        Err(error) => return Prepared::Output(output(2, error)),
    };
    let ParsedToolArguments::Invoke {
        command_path,
        input,
    } = parsed
    else {
        let ParsedToolArguments::Help(text) = parsed else {
            unreachable!()
        };
        return Prepared::Output(CommandOutput {
            exit_code: 0,
            stdout: text.into_bytes(),
            stderr: vec![],
        });
    };
    let Some(body) = tool
        .command_index_by_path(&command_path)
        .and_then(|index| tool.commands.nodes[index].body.as_ref())
    else {
        return Prepared::Output(output(2, "command has no body"));
    };
    let result_is_status = match body
        .result
        .as_ref()
        .map(|result| process_status_type(&tool.schema, &result.type_))
        .transpose()
    {
        Ok(status) => status.unwrap_or(false),
        Err(error) => return Prepared::Output(output(1, error)),
    };
    Prepared::Invoke(Box::new(PreparedCommandEntry {
        path: command_path,
        input: *input,
        stdin: body.stdin.is_some(),
        policy: ResultPolicy {
            stdout: body.stdout.is_some(),
            stderr: body.stderr.is_some(),
            result_is_status,
            errors: body
                .errors
                .iter()
                .map(|error| (error.name.clone(), error.exit_code))
                .collect(),
        },
    }))
}

pub(super) async fn prepare_command<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    name: String,
    args: Vec<String>,
    variables: Vec<(String, String)>,
) -> anyhow::Result<Option<CommandPreparation>> {
    // Discovery records only the tool name and deployment observation, not the
    // process environment. The parser uses only metadata-declared variables.
    let Some(tool) = ctx.get_tool_model(name).await? else {
        return Ok(None);
    };
    match prepare(&tool.definition, &args, &variables.into_iter().collect()) {
        Prepared::Output(output) => Ok(Some(CommandPreparation::Output(output))),
        Prepared::Invoke(command) => Ok(Some(CommandPreparation::Invoke(
            ctx.table().push(*command)?,
        ))),
    }
}

impl<Ctx: WorkerCtx> HostPreparedCommand for DurableWorkerCtx<Ctx> {
    async fn input(
        &mut self,
        self_: Resource<PreparedCommandEntry>,
    ) -> anyhow::Result<CommandInput> {
        let entry = self.table().get(&self_)?;
        let path = entry.path.clone();
        let input = entry.input.clone();
        let stdin = entry.stdin;
        let stdout = entry.policy.stdout;
        let stderr = entry.policy.stderr;
        Ok(CommandInput {
            command_path: path,
            input: encode_typed_tool_value(&input, self).map_err(anyhow::Error::msg)?,
            stdin,
            stdout,
            stderr,
        })
    }

    async fn render_result(
        &mut self,
        self_: Resource<PreparedCommandEntry>,
        result: Result<InvocationResult, ToolRpcError>,
    ) -> anyhow::Result<CommandOutput> {
        let policy = self.table().get(&self_)?.policy.clone();
        Ok(match result {
            Ok(result) => {
                let rendered = render_success(&policy, result.result, |value| {
                    decode_value_with(value, self)
                        .map_err(|_| "result: invalid typed value".to_string())
                });
                match rendered {
                    Ok(rendered) => CommandOutput {
                        exit_code: rendered.exit_code,
                        stdout: rendered.stdout.unwrap_or_default(),
                        stderr: vec![],
                    },
                    Err(error) => output(1, error),
                }
            }
            Err(error) => {
                let (status, message) = match error {
                    ToolRpcError::Denied(message) => (3, message),
                    ToolRpcError::NotFound(message) => (127, message),
                    ToolRpcError::Cancelled => (130, "Cancelled".into()),
                    ToolRpcError::RemoteToolError(
                        ToolError::InvalidInput(message) | ToolError::ConstraintViolation(message),
                    ) => (2, message),
                    ToolRpcError::RemoteToolError(ToolError::CustomError(custom)) => {
                        let decoded = decode_value_with(custom.payload.value, self).ok();
                        let payload = decoded.and_then(|value| {
                            decode_graph(&custom.payload.graph)
                                .ok()
                                .map(|graph| ModelTypedSchemaValue::new(graph, value))
                        });
                        render_custom_error(&custom.name, payload.as_ref(), &policy.errors)
                    }
                    ToolRpcError::ProtocolError(message)
                    | ToolRpcError::RemoteInternalError(message)
                    | ToolRpcError::ResourceExhausted(message) => (1, message),
                    other => (1, format!("{other:?}")),
                };
                output(status, message)
            }
        })
    }

    async fn drop(&mut self, self_: Resource<PreparedCommandEntry>) -> anyhow::Result<()> {
        self.table().delete(self_)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::tool::*;
    use test_r::test;

    #[test]
    fn ignored_and_invalid_result_graphs_release_transferred_handles() {
        use golem_common::schema::schema_value::SecretValuePayload;
        use golem_common::schema::wit::{SecretHandleRep, wire};
        for invalid_graph in [false, true] {
            let mut table = wasmtime::component::ResourceTable::new();
            let handle = table.push(SecretHandleRep::new(())).unwrap();
            let rep = handle.rep();
            let mut graph = encode_graph(&SchemaGraph::anonymous(SchemaType::secret(
                Default::default(),
            )))
            .unwrap();
            if invalid_graph {
                graph.root = i32::MAX;
            }
            let policy = ResultPolicy {
                stdout: true,
                stderr: false,
                result_is_status: false,
                errors: BTreeMap::new(),
            };
            let rendered = render_success(
                &policy,
                Some(TypedSchemaValue {
                    graph,
                    value: wire::SchemaValueTree {
                        root: 0,
                        value_nodes: vec![wire::SchemaValueNode::SecretValue(handle)],
                    },
                }),
                |value| {
                    let mut nodes = value.value_nodes.into_iter();
                    let Some(wire::SchemaValueNode::SecretValue(handle)) = nodes.next() else {
                        panic!()
                    };
                    assert!(nodes.next().is_none());
                    table.delete(handle).unwrap();
                    Ok(SchemaValue::Secret(SecretValuePayload {
                        secret_id: uuid::Uuid::nil(),
                        config_key: None,
                        version: 0,
                        resolved_at: chrono::DateTime::UNIX_EPOCH,
                        category: None,
                    }))
                },
            );
            assert!(
                table
                    .get(&Resource::<SecretHandleRep>::new_borrow(rep))
                    .is_err()
            );
            if invalid_graph {
                assert!(rendered.is_err());
            } else {
                assert_eq!(rendered.unwrap(), ProcessResult::default());
            }
        }
    }

    fn fixture() -> Tool {
        Tool {
            version: "0.0.1".into(),
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
                                name: "file".into(),
                                type_: SchemaType::string(),
                                doc: Doc::default(),
                                value_name: None,
                                default: None,
                                required: true,
                                accepts_stdio: false,
                            }],
                            ..Default::default()
                        },
                        options: vec![OptionSpec {
                            long: "cwd".into(),
                            short: None,
                            aliases: vec!["directory".into()],
                            doc: Doc::default(),
                            value_name: None,
                            env_var: Some("PWD".into()),
                            shape: OptionShape::Scalar(SchemaType::string()),
                            default: None,
                            required: true,
                        }],
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
    fn process_preparation_uses_the_canonical_parser_and_declared_variables() {
        let tool = fixture();
        let variables = HashMap::from([
            ("PWD".into(), "/workspace".into()),
            ("UNDECLARED".into(), "private".into()),
        ]);
        for args in [
            vec!["input.ts".into()],
            vec!["--directory=/override".into(), "input.ts".into()],
        ] {
            let ParsedToolArguments::Invoke {
                input,
                command_path,
            } = parse_with_variables(&tool, &args, &variables).unwrap()
            else {
                panic!()
            };
            let Prepared::Invoke(prepared) = prepare(&tool, &args, &variables) else {
                panic!()
            };
            assert_eq!(prepared.path, command_path);
            assert_eq!(prepared.input, *input);
            assert!(!format!("{:?}", prepared.input).contains("private"));
            assert!(!prepared.stdin);
            assert!(!prepared.policy.stdout);
            assert!(!prepared.policy.stderr);
        }
    }

    #[test]
    fn process_help_and_invalid_arguments_do_not_prepare_an_invocation() {
        let tool = fixture();
        let Prepared::Output(help) = prepare(&tool, &["--help".into()], &HashMap::new()) else {
            panic!()
        };
        assert_eq!(help.exit_code, 0);
        assert!(!help.stdout.is_empty());
        assert!(help.stderr.is_empty());
        let Prepared::Output(invalid) =
            prepare(&tool, &["--not-an-option".into()], &HashMap::new())
        else {
            panic!()
        };
        assert_eq!(invalid.exit_code, 2);
        assert!(invalid.stdout.is_empty());
        assert!(!invalid.stderr.is_empty());
    }
}
