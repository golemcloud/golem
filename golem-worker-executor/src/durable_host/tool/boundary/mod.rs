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
                result: Some(Box::new(typed)),
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
mod tests;
