// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
    Accessor, Arc, AttachmentController, AttachmentObserver, DurableWorkerCtx, FromSchema, HasSelf,
    HostResponseEntityInvocation, InvocationResult, ModelTypedSchemaValue,
    SerializableCustomToolError, SerializableEntityBodyExecution, SerializableToolError,
    SerializableToolInvocationResult, SerializableToolOperationTerminal,
    SerializableToolResultValue, SerializableToolRpcError, SerializableToolStructuredResult,
    ToolError, ToolInputDecodeFailure, ToolInvokeResponse, ToolOutputContract, ToolRpcError,
    TypedSchemaValue, WorkerCtx, WorkerExecutorError, decode_graph, decode_value_with,
    encode_graph, encode_tool_terminal, encode_value_with_streams, is_equivalent_cross_graph,
    validate_graph, validate_value,
};

pub(super) fn decode_typed_tool_value<Ctx: WorkerCtx>(
    value: TypedSchemaValue,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<ModelTypedSchemaValue, ToolInputDecodeFailure> {
    let graph =
        decode_graph(&value.graph).map_err(|_| ToolInputDecodeFailure::InvalidSchemaGraph)?;
    let decoded = decode_value_with(value.value, ctx)
        .map_err(|_| ToolInputDecodeFailure::InvalidSchemaValue)?;
    Ok(ModelTypedSchemaValue::new(graph, decoded))
}

pub(super) fn encode_typed_tool_value<Ctx: WorkerCtx>(
    value: &ModelTypedSchemaValue,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<TypedSchemaValue, String> {
    Ok(TypedSchemaValue {
        graph: encode_graph(value.graph()).map_err(|error| error.to_string())?,
        value: encode_value_with_streams(value.value(), ctx).map_err(|error| error.to_string())?,
    })
}

pub(super) fn project_tool_error<Ctx: WorkerCtx>(
    error: SerializableToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> ToolRpcError {
    let error = match error {
        SerializableToolError::InvalidToolName(value) => ToolError::InvalidToolName(value),
        SerializableToolError::InvalidCommandPath(value) => ToolError::InvalidCommandPath(value),
        SerializableToolError::InvalidInput(value) => ToolError::InvalidInput(value),
        SerializableToolError::ConstraintViolation(value) => ToolError::ConstraintViolation(value),
        SerializableToolError::InvalidResult(value) => ToolError::InvalidResult(value),
        SerializableToolError::CustomError(value) => {
            match encode_typed_tool_value(&value.payload, ctx) {
                Ok(payload) => {
                    ToolError::CustomError(golem_common::schema::wit::wire::CustomToolError {
                        name: value.name,
                        payload,
                    })
                }
                Err(error) => return ToolRpcError::ProtocolError(error),
            }
        }
    };
    ToolRpcError::RemoteToolError(error)
}

pub(super) fn project_tool_rpc_error<Ctx: WorkerCtx>(
    error: SerializableToolRpcError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> ToolRpcError {
    match error {
        SerializableToolRpcError::ProtocolError(value) => ToolRpcError::ProtocolError(value),
        SerializableToolRpcError::Denied(value) => ToolRpcError::Denied(value),
        SerializableToolRpcError::NotFound(value) => ToolRpcError::NotFound(value),
        SerializableToolRpcError::RemoteInternalError(value) => {
            ToolRpcError::RemoteInternalError(value)
        }
        SerializableToolRpcError::RemoteToolError(error) => project_tool_error(*error, ctx),
        SerializableToolRpcError::Cancelled => ToolRpcError::Cancelled,
        SerializableToolRpcError::ResourceExhausted(value) => {
            ToolRpcError::ResourceExhausted(value)
        }
    }
}

pub(super) fn project_tool_response_value<Ctx: WorkerCtx>(
    response: ToolInvokeResponse,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<TypedSchemaValue>, ToolRpcError> {
    response
        .map_err(|error| project_tool_rpc_error(error, ctx))
        .and_then(|response| {
            response
                .result
                .as_ref()
                .map(|value| encode_typed_tool_value(value, ctx))
                .transpose()
                .map_err(ToolRpcError::ProtocolError)
        })
}

pub(super) fn project_tool_response<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> Result<InvocationResult, ToolRpcError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let result = project_tool_response_value(response, access.get())?;
        Ok(InvocationResult {
            result,
            stdout: None,
            stderr: None,
        })
    })
}

pub(super) fn project_tool_unit<Ctx: WorkerCtx>(
    response: Result<(), SerializableToolRpcError>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(), ToolRpcError> {
    response.map_err(|error| project_tool_rpc_error(error, ctx))
}

pub(super) fn decode_tool_terminal(
    response: golem_common::model::oplog::HostResponseEntityInvocation,
) -> Result<ToolInvokeResponse, WorkerExecutorError> {
    let value = response.result.map_err(|error| {
        WorkerExecutorError::runtime(format!("entity invocation terminal failed: {error}"))
    })?;
    let terminal =
        SerializableToolOperationTerminal::from_value(value.value()).map_err(|error| {
            WorkerExecutorError::runtime(format!("invalid durable tool terminal: {error}"))
        })?;
    match terminal.result {
        Ok(result) => {
            let result = result
                .result
                .map(SerializableToolResultValue::into_typed)
                .transpose()
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "invalid durable tool result payload: {error}"
                    ))
                })?;
            Ok(Ok(SerializableToolInvocationResult {
                result: result.map(Box::new),
            }))
        }
        Err(error) => Ok(Err(error)),
    }
}

pub(super) fn decode_guest_tool_error<Ctx: WorkerCtx>(
    error: ToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> SerializableToolRpcError {
    let error = match error {
        ToolError::InvalidToolName(value) => SerializableToolError::InvalidToolName(value),
        ToolError::InvalidCommandPath(value) => SerializableToolError::InvalidCommandPath(value),
        ToolError::InvalidInput(value) => SerializableToolError::InvalidInput(value),
        ToolError::ConstraintViolation(value) => SerializableToolError::ConstraintViolation(value),
        ToolError::InvalidResult(value) => SerializableToolError::InvalidResult(value),
        ToolError::CustomError(value) => match decode_typed_tool_value(value.payload, ctx) {
            Ok(payload) => {
                SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                    name: value.name,
                    payload,
                }))
            }
            Err(_) => {
                return SerializableToolRpcError::ProtocolError(
                    "tool guest returned an invalid custom-error payload".to_string(),
                );
            }
        },
    };
    SerializableToolRpcError::RemoteToolError(Box::new(error))
}

pub(super) fn validate_declared_tool_error(
    error: SerializableToolRpcError,
    contract: &ToolOutputContract,
    allow_undeclared_custom_error: bool,
) -> SerializableToolRpcError {
    let SerializableToolRpcError::RemoteToolError(tool_error) = error else {
        return error;
    };
    let SerializableToolError::CustomError(custom) = tool_error.as_ref() else {
        return SerializableToolRpcError::RemoteToolError(tool_error);
    };
    let Some(declared) = contract.errors.iter().find(|case| case.name == custom.name) else {
        if allow_undeclared_custom_error
            && validate_graph(custom.payload.graph()).is_ok()
            && validate_value(
                custom.payload.graph(),
                &custom.payload.graph().root,
                custom.payload.value(),
            )
            .is_ok()
        {
            return SerializableToolRpcError::RemoteToolError(tool_error);
        }
        return SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult(format!(
                "tool returned undeclared custom error '{}'",
                custom.name
            )),
        ));
    };
    if !is_equivalent_cross_graph(
        custom.payload.graph(),
        &custom.payload.graph().root,
        &declared.payload,
        &declared.payload.root,
    ) || validate_value(
        custom.payload.graph(),
        &custom.payload.graph().root,
        custom.payload.value(),
    )
    .is_err()
    {
        return SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult(format!(
                "payload for custom error '{}' does not match its declared schema",
                custom.name
            )),
        ));
    }
    SerializableToolRpcError::RemoteToolError(tool_error)
}

pub(super) fn validate_declared_tool_result(
    result: Option<ModelTypedSchemaValue>,
    contract: &ToolOutputContract,
) -> Result<Option<ModelTypedSchemaValue>, SerializableToolRpcError> {
    match (result.as_ref(), contract.result.as_ref()) {
        (None, None) => Ok(result),
        (Some(value), Some(declared))
            if is_equivalent_cross_graph(
                value.graph(),
                &value.graph().root,
                declared,
                &declared.root,
            ) && validate_value(value.graph(), &value.graph().root, value.value()).is_ok() =>
        {
            Ok(result)
        }
        _ => Err(SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidResult(
                "tool result does not match the selected command's declared result".to_string(),
            ),
        ))),
    }
}

pub(super) fn validate_native_tool_output(
    result: Result<SerializableToolStructuredResult, SerializableToolRpcError>,
    contract: &ToolOutputContract,
) -> Result<SerializableToolStructuredResult, SerializableToolRpcError> {
    match result {
        Ok(result) => {
            let typed = result
                .result
                .clone()
                .map(SerializableToolResultValue::into_typed)
                .transpose()
                .map_err(|_| {
                    SerializableToolRpcError::RemoteToolError(Box::new(
                        SerializableToolError::InvalidResult(
                            "native tool returned an invalid result payload".to_string(),
                        ),
                    ))
                })?;
            validate_declared_tool_result(typed, contract)?;
            Ok(result)
        }
        Err(error) => Err(validate_declared_tool_error(error, contract, false)),
    }
}

pub(super) async fn encode_tool_operation_terminal(
    result: Result<SerializableToolStructuredResult, SerializableToolRpcError>,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        SerializableToolOperationTerminal {
            body_execution: SerializableEntityBodyExecution::Executed,
            result,
            admission_refusal: None,
        },
        "failed to encode durable tool operation terminal",
    )
    .await
}

pub(super) fn output_limit_error(
    channel: &str,
    host_resource_exhausted: bool,
) -> Option<SerializableToolRpcError> {
    host_resource_exhausted.then(|| {
        SerializableToolRpcError::ResourceExhausted(format!(
            "tool {channel} exceeded the attachment byte limit"
        ))
    })
}

pub(super) fn first_output_limit_error(
    stdout: Option<&AttachmentObserver>,
    stderr: Option<&AttachmentObserver>,
) -> Option<SerializableToolRpcError> {
    [("stdout", stdout), ("stderr", stderr)]
        .into_iter()
        .find_map(|(channel, observer)| {
            output_limit_error(
                channel,
                observer
                    .and_then(AttachmentObserver::terminal_snapshot)
                    .is_some_and(|terminal| terminal.host_resource_exhausted),
            )
        })
}

pub(super) fn publish_output_completions(
    stage_attachments: bool,
    stdout_completion_only: bool,
    stderr_completion_only: bool,
    stdout: Option<&AttachmentController>,
    stderr: Option<&AttachmentController>,
) {
    if (stage_attachments || stdout_completion_only)
        && let Some(stdout) = stdout
    {
        stdout.publish_completion();
    }
    if (stage_attachments || stderr_completion_only)
        && let Some(stderr) = stderr
    {
        stderr.publish_completion();
    }
}

pub(super) fn terminal_from_response(
    response: &HostResponseEntityInvocation,
) -> Result<Arc<SerializableToolOperationTerminal>, WorkerExecutorError> {
    let value = response.result.as_ref().map_err(|error| {
        WorkerExecutorError::runtime(format!("entity invocation terminal failed: {error}"))
    })?;
    SerializableToolOperationTerminal::from_value(value.value())
        .map(Arc::new)
        .map_err(|error| {
            WorkerExecutorError::runtime(format!("invalid durable tool terminal: {error}"))
        })
}
