// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::resolve_tool_command;
use crate::durable_host::entity::ResolvedEntityInvocationPosition;
use golem_common::model::entity::EntityInvocationPlanLayer;
use golem_common::model::oplog::payload::types::{
    SerializableCustomToolError, SerializableToolError, SerializableToolInvocationResult,
    SerializableToolRpcError,
};
use golem_common::schema::TypedSchemaValue as ModelTypedSchemaValue;
use golem_common::schema::tool::Tool;
use golem_common::schema::tool::compatibility::{
    CompiledCommandCompatibility, CompiledToolCompatibility, ProjectionStreamHandler,
    apply_projection,
};

/// The recorded projection edge selected for an underlying invocation.
///
/// `None` is the universal/wildcard boundary. It deliberately carries no
/// schema transformation, so opaque values and stream identities pass through
/// unchanged.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelectedToolProjectionEdge {
    command: Option<CompiledCommandCompatibility>,
    expected_error_names: Vec<String>,
}

impl SelectedToolProjectionEdge {
    fn identity() -> Self {
        Self {
            command: None,
            expected_error_names: Vec::new(),
        }
    }
}

/// The path and input to persist as the child invocation's replay identity,
/// together with the edge needed to project its response back to the caller.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PreparedUnderlyingToolCall {
    pub command_path: Vec<String>,
    pub input: ModelTypedSchemaValue,
    pub response_edge: SelectedToolProjectionEdge,
}

/// Validates an underlying call against the middleware's expected contract and
/// projects that call from the expected surface to the recorded inner surface.
pub(crate) fn prepare_underlying_tool_call(
    parent: &ResolvedEntityInvocationPosition,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    streams: &mut impl ProjectionStreamHandler,
) -> Result<PreparedUnderlyingToolCall, SerializableToolRpcError> {
    let EntityInvocationPlanLayer::Middleware {
        expected_definition,
        compatibility,
        ..
    } = parent.layer()
    else {
        return Err(invalid_result(
            "the terminal tool layer has no underlying projection boundary",
        ));
    };

    prepare_boundary(
        expected_definition.as_ref(),
        compatibility.as_ref(),
        command_path,
        input,
        streams,
    )
}

fn prepare_boundary(
    expected: Option<&Tool>,
    compatibility: Option<&CompiledToolCompatibility>,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    streams: &mut impl ProjectionStreamHandler,
) -> Result<PreparedUnderlyingToolCall, SerializableToolRpcError> {
    let Some(expected) = expected else {
        return Ok(PreparedUnderlyingToolCall {
            command_path,
            input,
            response_edge: SelectedToolProjectionEdge::identity(),
        });
    };

    let resolved = resolve_tool_command(expected, &command_path, &input)
        .map_err(|error| SerializableToolRpcError::RemoteToolError(Box::new(error)))?;
    let compatibility = compatibility.ok_or_else(|| {
        invalid_result("a monomorphic underlying boundary has no compiled compatibility plan")
    })?;
    let command = compatibility
        .commands
        .iter()
        .find(|command| {
            expected.command_index_by_path(&command.expected_path) == Some(resolved.command_index)
        })
        .cloned()
        .ok_or_else(|| {
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::InvalidCommandPath(command_path.clone()),
            ))
        })?;
    let body = expected.commands.nodes[resolved.command_index]
        .body
        .as_ref()
        .expect("resolved command has a body");
    let expected_error_names = body.errors.iter().map(|error| error.name.clone()).collect();
    let (_, value) = input.into_parts();
    let value = apply_projection(&command.input, value, streams)
        .map_err(|error| invalid_result(format_projection_error("input", error)))?;

    Ok(PreparedUnderlyingToolCall {
        command_path: command.inner_path.clone(),
        input: ModelTypedSchemaValue::new(command.input.target_schema.clone(), value),
        response_edge: SelectedToolProjectionEdge {
            command: Some(command),
            expected_error_names,
        },
    })
}

/// Projects an underlying response back to the middleware's expected surface.
pub(crate) fn project_underlying_tool_response(
    response: Result<SerializableToolInvocationResult, SerializableToolRpcError>,
    edge: &SelectedToolProjectionEdge,
    streams: &mut impl ProjectionStreamHandler,
) -> Result<SerializableToolInvocationResult, SerializableToolRpcError> {
    let Some(command) = &edge.command else {
        return response;
    };

    match response {
        Ok(result) => project_result(result, command, streams),
        Err(SerializableToolRpcError::RemoteToolError(error)) => {
            Err(project_tool_error(*error, command, edge, streams))
        }
        Err(error) => Err(error),
    }
}

fn project_result(
    result: SerializableToolInvocationResult,
    command: &CompiledCommandCompatibility,
    streams: &mut impl ProjectionStreamHandler,
) -> Result<SerializableToolInvocationResult, SerializableToolRpcError> {
    match (result.result, command.result.as_ref()) {
        (None, None) => Ok(SerializableToolInvocationResult { result: None }),
        (Some(typed), Some(plan)) => {
            let (_, value) = typed.into_parts();
            let value = apply_projection(plan, value, streams)
                .map_err(|error| invalid_result(format_projection_error("result", error)))?;
            let typed = ModelTypedSchemaValue::new(plan.target_schema.clone(), value);
            Ok(SerializableToolInvocationResult {
                result: Some(typed),
            })
        }
        _ => Err(invalid_result(
            "underlying result does not match the recorded projection plan",
        )),
    }
}

fn project_tool_error(
    error: SerializableToolError,
    command: &CompiledCommandCompatibility,
    edge: &SelectedToolProjectionEdge,
    streams: &mut impl ProjectionStreamHandler,
) -> SerializableToolRpcError {
    let SerializableToolError::CustomError(custom) = error else {
        return SerializableToolRpcError::RemoteToolError(Box::new(error));
    };
    let Some(projection) = command
        .errors
        .iter()
        .find(|projection| projection.name == custom.name)
    else {
        return if command.forward_unknown_errors {
            SerializableToolRpcError::RemoteToolError(Box::new(SerializableToolError::CustomError(
                custom,
            )))
        } else {
            invalid_result(format!(
                "underlying tool returned unknown custom error '{}'",
                custom.name
            ))
        };
    };
    let Some(expected_index) = projection.expected_index else {
        return invalid_result(format!(
            "underlying tool returned unmapped custom error '{}'",
            custom.name
        ));
    };
    let Some(expected_name) = edge.expected_error_names.get(expected_index) else {
        return invalid_result("recorded error projection has an invalid expected error index");
    };
    let payload = match projection.payload.as_ref() {
        Some(plan) => {
            let (_, value) = custom.payload.into_parts();
            match apply_projection(plan, value, streams) {
                Ok(value) => ModelTypedSchemaValue::new(plan.target_schema.clone(), value),
                Err(error) => {
                    return invalid_result(format_projection_error("custom error payload", error));
                }
            }
        }
        None => custom.payload,
    };
    SerializableToolRpcError::RemoteToolError(Box::new(SerializableToolError::CustomError(
        Box::new(SerializableCustomToolError {
            name: expected_name.clone(),
            payload,
        }),
    )))
}

fn format_projection_error(
    boundary: &str,
    error: golem_common::schema::tool::compatibility::ToolCompatibilityError,
) -> String {
    format!(
        "{boundary} projection failed at {}: {}",
        error.path, error.message
    )
}

fn invalid_result(message: impl Into<String>) -> SerializableToolRpcError {
    SerializableToolRpcError::RemoteToolError(Box::new(SerializableToolError::InvalidResult(
        message.into(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::SchemaGraph;
    use golem_common::schema::schema_type::NamedFieldType;
    use golem_common::schema::tool::compatibility::{
        ProjectionPlan, ToolCompatibilityMode, compile_tool_compatibility,
    };
    use golem_common::schema::tool::{
        CommandBody, CommandNode, CommandTree, ErrorCase, ErrorKind, Formatter, Globals,
        Positional, Positionals, ResultSpec,
    };
    use golem_common::schema::{SchemaType, SchemaValue};
    use golem_schema::schema::SchemaValueStream;
    use test_r::test;

    struct NoStreams;

    impl ProjectionStreamHandler for NoStreams {
        fn project_stream(
            &mut self,
            stream: SchemaValueStream,
            _item_plan: Option<ProjectionPlan>,
        ) -> Result<SchemaValueStream, String> {
            Ok(stream)
        }

        fn discard_stream(&mut self, _stream: SchemaValueStream) {}
    }

    fn record(names: &[&str]) -> SchemaType {
        SchemaType::record(
            names
                .iter()
                .map(|name| NamedFieldType {
                    name: (*name).into(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                })
                .collect(),
        )
    }

    fn tool(input: SchemaType, result: SchemaType, error_name: &str) -> Tool {
        Tool {
            version: "1".into(),
            schema: SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "outer".into(),
                    aliases: vec![],
                    doc: Default::default(),
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: Some(CommandBody {
                        positionals: Positionals {
                            fixed: vec![Positional {
                                name: "request".into(),
                                doc: Default::default(),
                                value_name: None,
                                type_: input,
                                default: None,
                                required: true,
                                accepts_stdio: false,
                            }],
                            tail: None,
                        },
                        options: vec![],
                        flags: vec![],
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        result: Some(ResultSpec {
                            type_: result,
                            doc: Default::default(),
                            formatters: vec![Formatter {
                                name: "json".into(),
                                doc: Default::default(),
                            }],
                            default_formatter: "json".into(),
                        }),
                        errors: vec![ErrorCase {
                            name: error_name.into(),
                            doc: Default::default(),
                            kind: ErrorKind::RuntimeError,
                            exit_code: 1,
                            payload: Some(record(&["left", "right"])),
                        }],
                        annotations: None,
                    }),
                }],
            },
        }
    }

    fn typed_record(names: &[&str], values: &[&str]) -> ModelTypedSchemaValue {
        ModelTypedSchemaValue::new(
            SchemaGraph::anonymous(record(names)),
            SchemaValue::Record {
                fields: values
                    .iter()
                    .map(|value| SchemaValue::String((*value).into()))
                    .collect(),
            },
        )
    }

    fn typed_input(request_names: &[&str], values: &[&str]) -> ModelTypedSchemaValue {
        ModelTypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(vec![NamedFieldType {
                name: "request".into(),
                body: record(request_names),
                metadata: Default::default(),
            }])),
            SchemaValue::Record {
                fields: vec![SchemaValue::Record {
                    fields: values
                        .iter()
                        .map(|value| SchemaValue::String((*value).into()))
                        .collect(),
                }],
            },
        )
    }

    #[test]
    fn wildcard_boundary_is_a_no_op() {
        let input = typed_record(&["opaque"], &["identity"]);
        let prepared = prepare_boundary(
            None,
            None,
            vec!["anything".into()],
            input.clone(),
            &mut NoStreams,
        )
        .unwrap();
        assert_eq!(prepared.command_path, vec!["anything"]);
        assert_eq!(prepared.input, input);

        let response = Err(SerializableToolRpcError::Cancelled);
        assert_eq!(
            project_underlying_tool_response(
                response.clone(),
                &prepared.response_edge,
                &mut NoStreams
            ),
            response
        );
    }

    #[test]
    fn projections_use_expected_to_inner_inputs_and_inner_to_expected_outputs() {
        let expected = tool(
            record(&["kept", "outer-owned"]),
            record(&["left", "right"]),
            "expected-error",
        );
        let inner = tool(
            record(&["kept"]),
            record(&["right", "left", "discarded"]),
            "expected-error",
        );
        let compatibility =
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
                .unwrap();
        let input = typed_input(&["kept", "outer-owned"], &["forwarded", "discarded"]);
        let prepared = prepare_boundary(
            Some(&expected),
            Some(&compatibility),
            vec![],
            input,
            &mut NoStreams,
        )
        .unwrap();
        assert_eq!(prepared.command_path, Vec::<String>::new());
        assert_eq!(
            prepared.input.value(),
            &SchemaValue::Record {
                fields: vec![SchemaValue::Record {
                    fields: vec![SchemaValue::String("forwarded".into())]
                }]
            }
        );

        let inner_result = typed_record(&["right", "left", "discarded"], &["R", "L", "ignored"]);
        let projected = project_underlying_tool_response(
            Ok(SerializableToolInvocationResult {
                result: Some(inner_result),
            }),
            &prepared.response_edge,
            &mut NoStreams,
        )
        .unwrap()
        .result
        .unwrap();
        assert_eq!(
            projected.value(),
            &SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("L".into()),
                    SchemaValue::String("R".into())
                ]
            }
        );

        let reversed = typed_record(&["left", "right"], &["L", "R"]);
        assert!(matches!(
            project_underlying_tool_response(
                Ok(SerializableToolInvocationResult {
                    result: Some(reversed)
                }),
                &prepared.response_edge,
                &mut NoStreams
            ),
            Err(SerializableToolRpcError::RemoteToolError(error))
                if matches!(*error, SerializableToolError::InvalidResult(_))
        ));
    }

    #[test]
    fn aliases_select_the_compiled_canonical_command() {
        let mut expected = tool(record(&["kept", "discarded"]), record(&[]), "error");
        let mut child = expected.commands.nodes[0].clone();
        child.name = "canonical".into();
        child.aliases = vec!["alias".into()];
        expected.commands.nodes[0].body = None;
        expected.commands.nodes[0].subcommands = vec![golem_common::schema::tool::CommandIndex(1)];
        expected.commands.nodes.push(child);
        let mut inner = expected.clone();
        inner.commands.nodes[1]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed[0]
            .type_ = record(&["kept"]);
        let compatibility =
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
                .unwrap();
        let prepared = prepare_boundary(
            Some(&expected),
            Some(&compatibility),
            vec!["alias".into()],
            typed_input(&["kept", "discarded"], &["forwarded", "ignored"]),
            &mut NoStreams,
        )
        .unwrap();
        assert_eq!(prepared.command_path, ["canonical"]);
        assert_eq!(
            prepared.input.value(),
            &SchemaValue::Record {
                fields: vec![SchemaValue::Record {
                    fields: vec![SchemaValue::String("forwarded".into())]
                }]
            }
        );
    }

    #[test]
    fn custom_errors_are_projected_and_unknown_inner_errors_are_rejected() {
        let expected = tool(record(&[]), record(&[]), "expected-error");
        let mut inner = tool(record(&[]), record(&[]), "expected-error");
        inner.commands.nodes[0].body.as_mut().unwrap().errors[0].payload =
            Some(record(&["right", "left", "discarded"]));
        let compatibility =
            compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
                .unwrap();
        let prepared = prepare_boundary(
            Some(&expected),
            Some(&compatibility),
            vec![],
            typed_input(&[], &[]),
            &mut NoStreams,
        )
        .unwrap();
        let error = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "expected-error".into(),
                payload: typed_record(&["right", "left", "discarded"], &["R", "L", "ignored"]),
            })),
        ));
        let Err(SerializableToolRpcError::RemoteToolError(projected)) =
            project_underlying_tool_response(Err(error), &prepared.response_edge, &mut NoStreams)
        else {
            panic!("custom error expected")
        };
        let SerializableToolError::CustomError(projected) = *projected else {
            panic!("custom error expected")
        };
        assert_eq!(projected.name, "expected-error");
        assert_eq!(
            projected.payload.value(),
            &SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("L".into()),
                    SchemaValue::String("R".into())
                ]
            }
        );

        let wrong_direction = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "unknown-inner-error".into(),
                payload: typed_record(&["left", "right"], &["L", "R"]),
            })),
        ));
        assert!(matches!(
            project_underlying_tool_response(
                Err(wrong_direction),
                &SelectedToolProjectionEdge {
                    command: Some(CompiledCommandCompatibility {
                        forward_unknown_errors: false,
                        ..compatibility.commands[0].clone()
                    }),
                    expected_error_names: vec!["expected-error".into()]
                },
                &mut NoStreams
            ),
            Err(SerializableToolRpcError::RemoteToolError(error))
                if matches!(*error, SerializableToolError::InvalidResult(_))
        ));
    }
}
