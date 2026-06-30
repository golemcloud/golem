// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::convert::Infallible;
use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::TypedSchemaValue;
use crate::bindings::golem::tool::host::RpcError as WitRpcError;
use crate::bindings::golem::tool::host::{self, ToolRpc as HostToolRpc};
use crate::schema::{FromSchema, FromSchemaError};
use crate::wasip2::io::streams::{InputStream, OutputStream};

/// RPC-level failures reported while invoking a remote tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RpcError {
    Protocol(String),
    Denied(String),
    NotFound(String),
    RemoteInternal(String),
}

impl Display for RpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Protocol(message) => write!(f, "protocol error: {message}"),
            RpcError::Denied(message) => write!(f, "denied: {message}"),
            RpcError::NotFound(message) => write!(f, "not found: {message}"),
            RpcError::RemoteInternal(message) => write!(f, "remote internal error: {message}"),
        }
    }
}

impl Error for RpcError {}

/// Failure returned by a typed tool client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolError<E> {
    Rpc(RpcError),
    Tool(E),
}

impl<E: Display> Display for ToolError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Rpc(error) => error.fmt(f),
            ToolError::Tool(error) => error.fmt(f),
        }
    }
}

impl<E: Error + 'static> Error for ToolError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ToolError::Rpc(error) => Some(error),
            ToolError::Tool(error) => Some(error),
        }
    }
}

/// Decoded successful result of `tool-rpc.invoke-and-await`.
pub struct InvocationResult {
    pub result: Option<TypedSchemaValue>,
    pub stdout: Option<OutputStream>,
}

/// Tool RPC resource types accepted by typed tool client helpers.
pub trait ToolRpcClient {
    fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<host::InvocationResult, WitRpcError>;
}

impl ToolRpcClient for HostToolRpc {
    fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<host::InvocationResult, WitRpcError> {
        self.invoke_and_await(command_path, input, stdin)
    }
}

impl ToolRpcClient for crate::golem_agentic::golem::tool::host::ToolRpc {
    fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<host::InvocationResult, WitRpcError> {
        self.invoke_and_await(command_path, input, stdin)
            .map(Into::into)
            .map_err(Into::into)
    }
}

/// Invokes a tool and decodes remote custom errors with a generated error decoder.
pub fn invoke_and_await<E>(
    rpc: &impl ToolRpcClient,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
    decode_error: impl Fn(TypedSchemaValue) -> Result<E, String>,
) -> Result<InvocationResult, ToolError<E>> {
    invoke_and_await_with_error_decoder(rpc, command_path, input, stdin, decode_error)
}

/// Invokes a tool whose remote custom-error payload is directly encoded as `E`.
pub fn invoke_and_await_payload_error<E: FromSchema>(
    rpc: &impl ToolRpcClient,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
) -> Result<InvocationResult, ToolError<E>> {
    invoke_and_await_with_error_decoder(
        rpc,
        command_path,
        input,
        stdin,
        decode_custom_tool_error::<E>,
    )
}

fn invoke_and_await_with_error_decoder<E>(
    rpc: &impl ToolRpcClient,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
    decode_error: impl Fn(TypedSchemaValue) -> Result<E, String>,
) -> Result<InvocationResult, ToolError<E>> {
    let input = crate::encode_typed_schema_value(input)
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin)
        .map_err(|error| map_rpc_error(error, &decode_error))?;

    let result_value = result
        .result
        .as_ref()
        .map(crate::decode_typed_schema_value)
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;

    Ok(InvocationResult {
        result: result_value,
        stdout: result.stdout,
    })
}

/// Invokes a zero-error tool and treats remote custom errors as protocol failures.
pub fn invoke_and_await_infallible(
    rpc: &impl ToolRpcClient,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
) -> Result<InvocationResult, ToolError<Infallible>> {
    let input = crate::encode_typed_schema_value(input)
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin)
        .map_err(map_infallible_rpc_error)?;

    let result_value = result
        .result
        .as_ref()
        .map(crate::decode_typed_schema_value)
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;

    Ok(InvocationResult {
        result: result_value,
        stdout: result.stdout,
    })
}

impl From<crate::golem_agentic::golem::tool::host::InvocationResult> for host::InvocationResult {
    fn from(result: crate::golem_agentic::golem::tool::host::InvocationResult) -> Self {
        Self {
            result: result.result,
            stdout: result.stdout,
        }
    }
}

impl From<crate::golem_agentic::golem::tool::host::RpcError> for WitRpcError {
    fn from(error: crate::golem_agentic::golem::tool::host::RpcError) -> Self {
        use crate::golem_agentic::golem::tool::host as agentic_host;

        match error {
            agentic_host::RpcError::ProtocolError(message) => Self::ProtocolError(message),
            agentic_host::RpcError::Denied(message) => Self::Denied(message),
            agentic_host::RpcError::NotFound(message) => Self::NotFound(message),
            agentic_host::RpcError::RemoteInternalError(message) => {
                Self::RemoteInternalError(message)
            }
            agentic_host::RpcError::RemoteToolError(error) => Self::RemoteToolError(error.into()),
        }
    }
}

impl From<crate::golem_agentic::golem::tool::host::ToolError> for host::ToolError {
    fn from(error: crate::golem_agentic::golem::tool::host::ToolError) -> Self {
        use crate::golem_agentic::golem::tool::host as agentic_host;

        match error {
            agentic_host::ToolError::InvalidToolName(name) => Self::InvalidToolName(name),
            agentic_host::ToolError::InvalidCommandPath(path) => Self::InvalidCommandPath(path),
            agentic_host::ToolError::InvalidInput(message) => Self::InvalidInput(message),
            agentic_host::ToolError::ConstraintViolation(message) => {
                Self::ConstraintViolation(message)
            }
            agentic_host::ToolError::InvalidResult(message) => Self::InvalidResult(message),
            agentic_host::ToolError::CustomError(value) => Self::CustomError(value),
        }
    }
}

fn map_rpc_error<E>(
    error: WitRpcError,
    decode_error: &impl Fn(TypedSchemaValue) -> Result<E, String>,
) -> ToolError<E> {
    match error {
        WitRpcError::ProtocolError(message) => ToolError::Rpc(RpcError::Protocol(message)),
        WitRpcError::Denied(message) => ToolError::Rpc(RpcError::Denied(message)),
        WitRpcError::NotFound(message) => ToolError::Rpc(RpcError::NotFound(message)),
        WitRpcError::RemoteInternalError(message) => {
            ToolError::Rpc(RpcError::RemoteInternal(message))
        }
        WitRpcError::RemoteToolError(error) => map_remote_tool_error(error, decode_error),
    }
}

fn map_infallible_rpc_error(error: WitRpcError) -> ToolError<Infallible> {
    match error {
        WitRpcError::ProtocolError(message) => ToolError::Rpc(RpcError::Protocol(message)),
        WitRpcError::Denied(message) => ToolError::Rpc(RpcError::Denied(message)),
        WitRpcError::NotFound(message) => ToolError::Rpc(RpcError::NotFound(message)),
        WitRpcError::RemoteInternalError(message) => {
            ToolError::Rpc(RpcError::RemoteInternal(message))
        }
        WitRpcError::RemoteToolError(error) => ToolError::Rpc(RpcError::Protocol(format!(
            "remote tool error: {}",
            remote_tool_error_label(&error)
        ))),
    }
}

fn map_remote_tool_error<E>(
    error: host::ToolError,
    decode_error: &impl Fn(TypedSchemaValue) -> Result<E, String>,
) -> ToolError<E> {
    match error {
        host::ToolError::CustomError(value) => match decode_custom_tool_error_value(&value) {
            Ok(value) => match decode_error(value) {
                Ok(error) => ToolError::Tool(error),
                Err(message) => ToolError::Rpc(RpcError::Protocol(message)),
            },
            Err(message) => ToolError::Rpc(RpcError::Protocol(message)),
        },
        error => ToolError::Rpc(RpcError::Protocol(format!(
            "remote tool error: {}",
            remote_tool_error_label(&error)
        ))),
    }
}

fn decode_custom_tool_error<E: FromSchema>(value: TypedSchemaValue) -> Result<E, String> {
    E::from_value(value.value()).map_err(format_from_schema_error)
}

fn decode_custom_tool_error_value(
    value: &crate::schema::wit::wire::TypedSchemaValue,
) -> Result<TypedSchemaValue, String> {
    crate::decode_typed_schema_value(value)
        .map_err(|error| format!("failed to decode remote tool error: {error}"))
}

fn format_from_schema_error(error: FromSchemaError) -> String {
    format!("failed to decode remote tool error: {error}")
}

fn protocol_error<E>(message: String) -> ToolError<E> {
    ToolError::Rpc(RpcError::Protocol(message))
}

fn remote_tool_error_label(error: &host::ToolError) -> String {
    match error {
        host::ToolError::InvalidToolName(name) => format!("invalid tool name `{name}`"),
        host::ToolError::InvalidCommandPath(path) => {
            format!("invalid command path `{}`", path.join(" "))
        }
        host::ToolError::InvalidInput(message) => format!("invalid input: {message}"),
        host::ToolError::ConstraintViolation(message) => {
            format!("constraint violation: {message}")
        }
        host::ToolError::InvalidResult(message) => format!("invalid result: {message}"),
        host::ToolError::CustomError(_) => "custom error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FromSchema, IntoSchema, IntoTypedSchemaValue};
    use test_r::test;

    #[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
    enum CliError {
        Usage(String),
    }

    #[test]
    fn custom_tool_error_payload_decodes_to_declared_error_variant() {
        let payload = "bad flag".to_string().into_typed_schema_value().unwrap();
        let wire_payload = crate::encode_typed_schema_value(&payload).unwrap();

        let decoded = map_remote_tool_error(host::ToolError::CustomError(wire_payload), &|value| {
            String::from_value(value.value())
                .map(CliError::Usage)
                .map_err(format_from_schema_error)
        });

        assert_eq!(
            decoded,
            ToolError::Tool(CliError::Usage("bad flag".to_string()))
        );
    }

    struct FakeToolRpc;

    impl ToolRpcClient for FakeToolRpc {
        fn invoke_and_await_tool(
            &self,
            _command_path: &[String],
            _input: crate::schema::wit::wire::TypedSchemaValue,
            _stdin: Option<InputStream>,
        ) -> Result<host::InvocationResult, WitRpcError> {
            let payload = "bad flag".to_string().into_typed_schema_value().unwrap();
            let wire_payload = crate::encode_typed_schema_value(&payload).unwrap();

            Err(WitRpcError::RemoteToolError(host::ToolError::CustomError(
                wire_payload,
            )))
        }
    }

    #[test]
    fn invoke_and_await_decoding_error_decodes_custom_tool_error_payload() {
        let input = ().into_typed_schema_value().unwrap();

        let decode_error = |value: TypedSchemaValue| {
            String::from_value(value.value())
                .map(CliError::Usage)
                .map_err(format_from_schema_error)
        };

        match invoke_and_await(&FakeToolRpc, &[], &input, None, decode_error) {
            Err(ToolError::Tool(CliError::Usage(message))) => assert_eq!(message, "bad flag"),
            Err(ToolError::Rpc(error)) => {
                panic!("expected declared tool error, got RPC error: {error:?}")
            }
            Ok(_) => panic!("expected declared tool error, got success"),
        }
    }
}
