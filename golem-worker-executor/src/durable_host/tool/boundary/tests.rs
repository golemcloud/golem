// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::*;
use golem_common::schema::SchemaGraph;
use golem_common::schema::schema_type::NamedFieldType;
use golem_common::schema::tool::compatibility::{
    ProjectionPlan, ToolCompatibilityMode, compile_tool_compatibility,
};
use golem_common::schema::tool::{
    CommandBody, CommandNode, CommandTree, ErrorCase, ErrorKind, Formatter, Globals, Positional,
    Positionals, ResultSpec,
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
        requires_filesystem: false,
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
                    stderr: None,
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
        project_underlying_tool_response(response.clone(), &prepared.response_edge, &mut NoStreams),
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
            result: Some(Box::new(inner_result)),
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
                result: Some(Box::new(reversed))
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
            &prepared.response_edge,
            &mut NoStreams
        ),
        Err(SerializableToolRpcError::RemoteToolError(error))
            if matches!(*error, SerializableToolError::InvalidResult(_))
    ));
}
