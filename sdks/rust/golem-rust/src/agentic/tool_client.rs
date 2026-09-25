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

#[cfg(test)]
use std::cell::RefCell;
use std::convert::Infallible;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::Future;
#[cfg(test)]
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Rc;
#[cfg(test)]
use std::sync::Arc;
use std::task::{Context, Poll};
#[cfg(test)]
use std::task::{Wake, Waker};

use crate::TypedSchemaValue;
use crate::agentic::AmbientToolRpc;
use crate::agentic::DirectToolError;
use crate::agentic::InputStream;
use crate::bindings::golem::tool::host::{
    self, ToolRpc as HostToolRpc, ToolStdin as HostToolStdin, ToolStdout as HostToolStdout,
};
use crate::golem_agentic::golem::tool::host as agentic_host_api;
use crate::schema::validation::subtyping::is_equivalent_cross_graph;
use crate::schema::wit::wire::{ToolError as WitToolError, ToolRpcError as WitRpcError};
use crate::schema::{FromSchema, FromSchemaError, IntoSchema};
use crate::tool::RawCustomToolError;

/// RPC-level failures reported while invoking a remote tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RpcError {
    Protocol(String),
    Denied(String),
    NotFound(String),
    RemoteInternal(String),
    Cancelled,
    ResourceExhausted(String),
}

impl Display for RpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Protocol(message) => write!(f, "protocol error: {message}"),
            RpcError::Denied(message) => write!(f, "denied: {message}"),
            RpcError::NotFound(message) => write!(f, "not found: {message}"),
            RpcError::RemoteInternal(message) => write!(f, "remote internal error: {message}"),
            RpcError::Cancelled => write!(f, "cancelled"),
            RpcError::ResourceExhausted(message) => {
                write!(f, "resource exhausted: {message}")
            }
        }
    }
}

impl Error for RpcError {}

/// Failure returned by a typed tool client.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ToolError<E> {
    Rpc(RpcError),
    RemoteTool(RemoteToolError),
    Tool(E),
    UnknownCustomError(RawCustomToolError),
    MalformedRemoteOutput(String),
}

/// A structural failure returned by the remote tool implementation.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteToolError {
    InvalidToolName(String),
    InvalidCommandPath(Vec<String>),
    InvalidInput(String),
    ConstraintViolation(String),
    InvalidResult(String),
}

/// Generated marker for a tool trait method that is invokable as a command body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolLeafCommand;

/// Generated marker for a tool trait method that only grafts a subtree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolSubtreeCommand;

#[doc(hidden)]
pub struct OmittedSurface<const ID: u64>;

#[doc(hidden)]
pub trait ToolClientWithParts: Sized {
    fn __golem_tool_client_with_parts(
        root_tool_name: String,
        command_path: Vec<String>,
        schema_path: Vec<String>,
        inherited_prefix: Vec<DirectInputValue>,
    ) -> Self;
}

trait DirectInputEncoder {
    fn preflight(
        &self,
        preflight: &mut crate::schema::wit::direct::WirePreflight,
    ) -> Result<(), crate::schema::wit::direct::WireError>;
    fn prepare(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::schema::wit::direct::WireError>> + '_>>;
    fn write(
        &self,
        writer: &mut crate::schema::wit::direct::WireWriter,
    ) -> Result<crate::schema::wit::wire::ValueNodeIndex, crate::schema::wit::direct::WireError>;
    fn schema(
        &self,
        builder: &mut crate::schema::wit::direct::WireSchemaBuilder,
    ) -> crate::schema::wit::wire::TypeNodeIndex;
}

impl<T: crate::schema::wit::direct::IntoWire + crate::schema::wit::direct::WireSchema>
    DirectInputEncoder for T
{
    fn preflight(
        &self,
        preflight: &mut crate::schema::wit::direct::WirePreflight,
    ) -> Result<(), crate::schema::wit::direct::WireError> {
        crate::schema::wit::direct::IntoWire::preflight(self, preflight)
    }
    fn prepare(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::schema::wit::direct::WireError>> + '_>> {
        Box::pin(crate::schema::wit::direct::IntoWire::prepare_wire(self))
    }
    fn write(
        &self,
        writer: &mut crate::schema::wit::direct::WireWriter,
    ) -> Result<crate::schema::wit::wire::ValueNodeIndex, crate::schema::wit::direct::WireError>
    {
        crate::schema::wit::direct::IntoWire::write_wire(self, writer)
    }
    fn schema(
        &self,
        builder: &mut crate::schema::wit::direct::WireSchemaBuilder,
    ) -> crate::schema::wit::wire::TypeNodeIndex {
        T::append_schema(builder)
    }
}

#[derive(Clone)]
#[doc(hidden)]
pub struct DirectInputValue {
    pub name: String,
    pub aliases: Vec<String>,
    pub short: Option<char>,
    option_carrier: bool,
    encoder: Rc<dyn DirectInputEncoder>,
}

impl DirectInputValue {
    pub fn new<
        T: crate::schema::wit::direct::IntoWire + crate::schema::wit::direct::WireSchema + 'static,
    >(
        name: impl Into<String>,
        aliases: Vec<String>,
        short: Option<char>,
        value: T,
    ) -> Self {
        Self {
            name: name.into(),
            aliases,
            short,
            option_carrier: false,
            encoder: Rc::new(value),
        }
    }

    pub fn with_option_carrier(mut self, option_carrier: bool) -> Self {
        self.option_carrier = option_carrier;
        self
    }
}

#[doc(hidden)]
pub async fn encode_direct_tool_input(
    values: &[DirectInputValue],
    field_order: &[&str],
) -> Result<crate::schema::wit::wire::TypedSchemaValue, String> {
    use crate::schema::wit::direct::{
        WirePreflight, WireSchemaBuilder, WireWriter, empty_metadata,
    };
    let mut selected: Vec<&DirectInputValue> = Vec::new();
    for value in values {
        if let Some(previous) = selected
            .iter_mut()
            .find(|previous| previous.name == value.name)
        {
            *previous = value;
        } else {
            selected.push(value);
        }
    }
    selected.sort_by_key(|value| {
        field_order
            .iter()
            .position(|name| *name == value.name)
            .map(|index| index + 1)
            .unwrap_or(0)
    });
    let values = selected.as_slice();
    let mut preflight = WirePreflight::asynchronous();
    for value in values {
        value
            .encoder
            .preflight(&mut preflight)
            .map_err(|e| e.to_string())?;
    }
    for value in values {
        value.encoder.prepare().await.map_err(|e| e.to_string())?;
    }
    let mut schemas = WireSchemaBuilder::default();
    let mut fields = Vec::with_capacity(values.len());
    let mut encoded_are_options = Vec::with_capacity(values.len());
    for value in values {
        let mut metadata = empty_metadata();
        metadata.aliases = value.aliases.clone();
        let body = value.encoder.schema(&mut schemas);
        let body_is_option = matches!(
            schemas.resolve(body).map(|node| &node.body),
            Some(crate::schema::wit::wire::SchemaTypeBody::OptionType(_))
        );
        encoded_are_options.push(body_is_option);
        let body = if value.option_carrier && !body_is_option {
            schemas.push(crate::schema::wit::wire::SchemaTypeBody::OptionType(body))
        } else {
            body
        };
        fields.push(crate::schema::wit::wire::NamedFieldType {
            name: value.name.clone(),
            body,
            metadata,
        });
    }
    let schema_root = schemas.push(crate::schema::wit::wire::SchemaTypeBody::RecordType(fields));
    let graph = schemas.finish(schema_root);
    let mut writer = WireWriter::default();
    let mut indices = Vec::with_capacity(values.len());
    for (value, encoded_is_option) in values.iter().zip(encoded_are_options) {
        let index = value
            .encoder
            .write(&mut writer)
            .map_err(|e| e.to_string())?;
        indices.push(if value.option_carrier && !encoded_is_option {
            writer.push(crate::schema::wit::wire::SchemaValueNode::OptionValue(
                Some(index),
            ))
        } else {
            index
        });
    }
    let root = writer.push(crate::schema::wit::wire::SchemaValueNode::RecordValue(
        indices,
    ));
    Ok(crate::schema::wit::wire::TypedSchemaValue {
        graph,
        value: writer.finish(root),
    })
}

impl<E: Display> Display for ToolError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Rpc(error) => error.fmt(f),
            ToolError::RemoteTool(error) => write!(f, "remote tool error: {error}"),
            ToolError::Tool(error) => error.fmt(f),
            ToolError::UnknownCustomError(error) => {
                write!(f, "unknown custom tool error `{}`", error.name)
            }
            ToolError::MalformedRemoteOutput(message) => {
                write!(f, "malformed remote tool output: {message}")
            }
        }
    }
}

impl<E: Error + 'static> Error for ToolError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ToolError::Rpc(error) => Some(error),
            ToolError::RemoteTool(error) => Some(error),
            ToolError::Tool(error) => Some(error),
            ToolError::UnknownCustomError(_) => None,
            ToolError::MalformedRemoteOutput(_) => None,
        }
    }
}

impl Display for RemoteToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteToolError::InvalidToolName(name) => write!(f, "invalid tool name `{name}`"),
            RemoteToolError::InvalidCommandPath(path) => {
                write!(f, "invalid command path `{}`", path.join(" "))
            }
            RemoteToolError::InvalidInput(message) => write!(f, "invalid input: {message}"),
            RemoteToolError::ConstraintViolation(message) => {
                write!(f, "constraint violation: {message}")
            }
            RemoteToolError::InvalidResult(message) => write!(f, "invalid result: {message}"),
        }
    }
}

impl Error for RemoteToolError {}

/// Decoded successful result of `tool-rpc.invoke-and-await`.
#[derive(Clone)]
pub struct InvocationResult {
    pub result: Option<TypedSchemaValue>,
}

/// Direct wire completion used by generated typed clients.
#[derive(Clone)]
pub struct DirectInvocationResult {
    pub snapshot: Rc<crate::schema::wit::direct::WireSnapshot>,
    pub root: Option<crate::schema::wit::wire::ValueNodeIndex>,
}

pub fn decode_direct_result_value<T: crate::schema::wit::direct::FromWire, E>(
    result: DirectInvocationResult,
) -> Result<T, ToolError<E>> {
    let root = result
        .root
        .ok_or_else(|| tool_protocol_error("tool result did not contain a value"))?;
    let mut reader = result.snapshot.reader();
    let value = T::read_wire(&mut reader, root).map_err(|e| tool_protocol_error(e.to_string()))?;
    reader
        .finish()
        .map_err(|e| tool_protocol_error(e.to_string()))?;
    Ok(value)
}

pub fn decode_direct_result_empty<E>(result: DirectInvocationResult) -> Result<(), ToolError<E>> {
    if result.root.is_some() {
        Err(tool_protocol_error(
            "tool result unexpectedly contained a value",
        ))
    } else {
        Ok(())
    }
}

pub async fn invoke_and_await_direct<E: DirectToolError, R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: crate::schema::wit::wire::TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
) -> Result<DirectInvocationResult, ToolError<E>> {
    invoke_and_await_direct_with_error_decoder(
        rpc,
        command_path,
        input,
        stdin,
        stdout,
        E::recognizes_error_name,
        E::from_direct_error_reader,
    )
    .await
}

pub async fn invoke_and_await_direct_with_error_decoder<E, R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: crate::schema::wit::wire::TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
    recognizes_error: fn(&str) -> bool,
    decode_error: impl Fn(
        &str,
        &mut crate::schema::wit::direct::WireReader,
        i32,
    ) -> Result<Option<E>, String>,
) -> Result<DirectInvocationResult, ToolError<E>> {
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(|error| {
            decode_cached_direct_error(cache_direct_error(error, recognizes_error), &decode_error)
        })?;
    decode_direct_wire_invocation_result(result)
}

pub async fn invoke_and_await_direct_infallible<R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: crate::schema::wit::wire::TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
) -> Result<DirectInvocationResult, ToolError<Infallible>> {
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(map_infallible_rpc_error)?;
    decode_direct_wire_invocation_result(result)
}

fn decode_direct_wire_invocation_result<E>(
    result: host::InvocationResult,
) -> Result<DirectInvocationResult, ToolError<E>> {
    if result.stdout.is_some() {
        return Err(tool_protocol_error(
            "tool result unexpectedly contained an embedded stdout stream",
        ));
    }
    match result.result {
        Some(value) => Ok(DirectInvocationResult {
            root: Some(value.value.root),
            snapshot: Rc::new(crate::schema::wit::direct::WireSnapshot::new(
                value.value.value_nodes,
            )),
        }),
        None => Ok(DirectInvocationResult {
            root: None,
            snapshot: Rc::new(crate::schema::wit::direct::WireSnapshot::new(Vec::new())),
        }),
    }
}

/// Decodes a structured invocation result and pairs it with its independently
/// acquired stdout stream.
pub fn decode_result_with_stdout<T: FromSchema + IntoSchema, E>(
    result: InvocationResult,
    stdout: InputStream,
) -> Result<(T, InputStream), ToolError<E>> {
    let value = decode_expected_value(result.result)?;
    Ok((value, stdout))
}

/// Decodes an invocation result declared to carry a value.
pub fn decode_result_value<T: FromSchema + IntoSchema, E>(
    result: InvocationResult,
) -> Result<T, ToolError<E>> {
    decode_expected_value(result.result)
}

/// Validates a stdout-only structured terminal and returns its independently
/// acquired stream.
pub fn decode_result_stdout_only<E>(
    result: InvocationResult,
    stdout: InputStream,
) -> Result<InputStream, ToolError<E>> {
    expect_no_value(result.result)?;
    Ok(stdout)
}

/// Decodes an invocation result declared to carry no value.
pub fn decode_result_empty<E>(result: InvocationResult) -> Result<(), ToolError<E>> {
    expect_no_value(result.result)
}

fn decode_expected_value<T: FromSchema + IntoSchema, E>(
    value: Option<TypedSchemaValue>,
) -> Result<T, ToolError<E>> {
    let value = expect_value(value)?;
    let expected = crate::schema::try_into_schema_graph::<T>()
        .map_err(|error| protocol_error(error.to_string()))?;
    if !is_equivalent_cross_graph(
        value.graph(),
        &value.graph().root,
        &expected,
        &expected.root,
    ) {
        return Err(protocol_error(
            "tool result schema does not match the expected result schema".to_string(),
        ));
    }
    T::from_value(value.value()).map_err(|error| protocol_error(error.to_string()))
}

/// Validates a custom error payload against the caller-owned error declaration before decoding it.
pub fn decode_declared_tool_error<E: super::ToolErrorSchema>(
    name: String,
    value: TypedSchemaValue,
) -> Result<Option<E>, String> {
    let cases = E::error_cases().map_err(|error| error.to_string())?;
    let Some(case) = cases.iter().find(|case| case.name == name) else {
        return Ok(None);
    };
    match &case.payload {
        Some(expected) => {
            if !is_equivalent_cross_graph(
                value.graph(),
                &value.graph().root,
                expected,
                &expected.root,
            ) {
                return Err(format!("custom error `{name}` has the wrong schema"));
            }
            crate::schema::validation::validate_value(expected, &expected.root, value.value())
                .map_err(|errors| {
                    errors
                        .into_iter()
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                })?;
        }
        None if !matches!(value.value(), crate::SchemaValue::Tuple { elements } if elements.is_empty()) =>
        {
            return Err(format!("custom error `{name}` has an unexpected payload"));
        }
        None => {}
    }
    E::from_error_payload_value(name, value)
}

/// Requires the declared result value to be present in an invocation result.
pub fn expect_value<E>(value: Option<TypedSchemaValue>) -> Result<TypedSchemaValue, ToolError<E>> {
    value.ok_or_else(|| protocol_error("tool result did not contain a value".to_string()))
}

/// Maps a client-side encode/decode failure message onto the protocol error
/// variant of [`ToolError`].
pub fn tool_protocol_error<E>(message: impl Into<String>) -> ToolError<E> {
    protocol_error(message.into())
}

/// Rejects an invocation result that unexpectedly carries a value.
pub fn expect_no_value<E>(value: Option<TypedSchemaValue>) -> Result<(), ToolError<E>> {
    if value.is_some() {
        return Err(protocol_error(
            "tool result unexpectedly contained a value".to_string(),
        ));
    }
    Ok(())
}

/// Tool RPC resource types accepted by typed tool client helpers.
#[allow(async_fn_in_trait)]
pub trait ToolRpcClient {
    type Stdin;
    type Stdout;

    async fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<Self::Stdin>,
        stdout: Option<Self::Stdout>,
    ) -> Result<host::InvocationResult, WitRpcError>;
}

#[doc(hidden)]
pub trait StartedToolRpcClient {
    fn async_invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<agentic_host_api::ToolStdin>,
        stdout: Option<agentic_host_api::ToolStdout>,
    ) -> agentic_host_api::FutureInvokeResult;
}

impl ToolRpcClient for HostToolRpc {
    type Stdin = HostToolStdin;
    type Stdout = HostToolStdout;

    async fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<Self::Stdin>,
        stdout: Option<Self::Stdout>,
    ) -> Result<host::InvocationResult, WitRpcError> {
        self.invoke_and_await(command_path.to_vec(), input, stdin, stdout)
            .await
    }
}

impl ToolRpcClient for AmbientToolRpc {
    type Stdin = agentic_host_api::ToolStdin;
    type Stdout = agentic_host_api::ToolStdout;

    async fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<Self::Stdin>,
        stdout: Option<Self::Stdout>,
    ) -> Result<host::InvocationResult, WitRpcError> {
        self.inner
            .invoke_and_await(command_path.to_vec(), input, stdin, stdout)
            .await
    }
}

impl StartedToolRpcClient for AmbientToolRpc {
    fn async_invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<agentic_host_api::ToolStdin>,
        stdout: Option<agentic_host_api::ToolStdout>,
    ) -> agentic_host_api::FutureInvokeResult {
        self.inner
            .async_invoke_and_await(command_path, input, stdin, stdout)
    }
}

impl ToolRpcClient for crate::golem_agentic::golem::tool::host::ToolRpc {
    type Stdin = crate::golem_agentic::golem::tool::host::ToolStdin;
    type Stdout = crate::golem_agentic::golem::tool::host::ToolStdout;

    async fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<Self::Stdin>,
        stdout: Option<Self::Stdout>,
    ) -> Result<host::InvocationResult, WitRpcError> {
        self.invoke_and_await(command_path.to_vec(), input, stdin, stdout)
            .await
    }
}

impl StartedToolRpcClient for crate::golem_agentic::golem::tool::host::ToolRpc {
    fn async_invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<agentic_host_api::ToolStdin>,
        stdout: Option<agentic_host_api::ToolStdout>,
    ) -> agentic_host_api::FutureInvokeResult {
        self.async_invoke_and_await(command_path, input, stdin, stdout)
    }
}

/// Invokes a tool and decodes remote custom errors with a generated error decoder.
pub async fn invoke_and_await<E, R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
    decode_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
) -> Result<InvocationResult, ToolError<E>> {
    invoke_and_await_with_error_decoder(rpc, command_path, input, stdin, stdout, decode_error).await
}

/// Invokes a tool whose remote custom-error payload is directly encoded as `E`.
pub async fn invoke_and_await_payload_error<E: FromSchema, R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
) -> Result<InvocationResult, ToolError<E>> {
    invoke_and_await_with_error_decoder(
        rpc,
        command_path,
        input,
        stdin,
        stdout,
        decode_custom_tool_error::<E>,
    )
    .await
}

async fn invoke_and_await_with_error_decoder<E, R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
    decode_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
) -> Result<InvocationResult, ToolError<E>> {
    let input = crate::encode_typed_schema_value_async(input)
        .await
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(|error| map_rpc_error(error, &decode_error))?;

    decode_wire_invocation_result(result)
}

/// Invokes a zero-error tool and treats remote custom errors as protocol failures.
pub async fn invoke_and_await_infallible<R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
) -> Result<InvocationResult, ToolError<Infallible>> {
    let input = crate::encode_typed_schema_value_async(input)
        .await
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(map_infallible_rpc_error)?;

    decode_wire_invocation_result(result)
}

pub(crate) fn map_rpc_error<E>(
    error: WitRpcError,
    decode_error: &(impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String> + ?Sized),
) -> ToolError<E> {
    match error {
        WitRpcError::ProtocolError(message) => ToolError::Rpc(RpcError::Protocol(message)),
        WitRpcError::Denied(message) => ToolError::Rpc(RpcError::Denied(message)),
        WitRpcError::NotFound(message) => ToolError::Rpc(RpcError::NotFound(message)),
        WitRpcError::RemoteInternalError(message) => {
            ToolError::Rpc(RpcError::RemoteInternal(message))
        }
        WitRpcError::RemoteToolError(error) => map_remote_tool_error(error, decode_error),
        WitRpcError::Cancelled => ToolError::Rpc(RpcError::Cancelled),
        WitRpcError::ResourceExhausted(message) => {
            ToolError::Rpc(RpcError::ResourceExhausted(message))
        }
    }
}

fn map_infallible_rpc_error<E>(error: WitRpcError) -> ToolError<E> {
    match error {
        WitRpcError::ProtocolError(message) => ToolError::Rpc(RpcError::Protocol(message)),
        WitRpcError::Denied(message) => ToolError::Rpc(RpcError::Denied(message)),
        WitRpcError::NotFound(message) => ToolError::Rpc(RpcError::NotFound(message)),
        WitRpcError::RemoteInternalError(message) => {
            ToolError::Rpc(RpcError::RemoteInternal(message))
        }
        WitRpcError::RemoteToolError(error) => map_remote_tool_error(error, &|_, _| Ok(None)),
        WitRpcError::Cancelled => ToolError::Rpc(RpcError::Cancelled),
        WitRpcError::ResourceExhausted(message) => {
            ToolError::Rpc(RpcError::ResourceExhausted(message))
        }
    }
}

async fn join<A, B>(left: impl Future<Output = A>, right: impl Future<Output = B>) -> (A, B) {
    struct Join<L, R, A, B> {
        left: Option<Pin<Box<L>>>,
        right: Option<Pin<Box<R>>>,
        left_value: Option<A>,
        right_value: Option<B>,
    }
    impl<A, B, L: Future<Output = A>, R: Future<Output = B>> Future for Join<L, R, A, B> {
        type Output = (A, B);

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            // The futures remain pinned in their boxes; the surrounding fields
            // are never structurally pinned.
            let this = unsafe { self.get_unchecked_mut() };
            if this.left_value.is_none()
                && let Poll::Ready(value) = this.left.as_mut().unwrap().as_mut().poll(cx)
            {
                this.left = None;
                this.left_value = Some(value);
            }
            if this.right_value.is_none()
                && let Poll::Ready(value) = this.right.as_mut().unwrap().as_mut().poll(cx)
            {
                this.right = None;
                this.right_value = Some(value);
            }
            if this.left_value.is_some() && this.right_value.is_some() {
                Poll::Ready((
                    this.left_value.take().unwrap(),
                    this.right_value.take().unwrap(),
                ))
            } else {
                Poll::Pending
            }
        }
    }
    Join {
        left: Some(Box::pin(left)),
        right: Some(Box::pin(right)),
        left_value: None,
        right_value: None,
    }
    .await
}

async fn drive_left_until_right<A, B>(
    left: impl Future<Output = A>,
    right: impl Future<Output = B>,
) -> B {
    struct Drive<L, R> {
        left: Option<Pin<Box<L>>>,
        right: Pin<Box<R>>,
    }
    impl<A, B, L: Future<Output = A>, R: Future<Output = B>> Future for Drive<L, R> {
        type Output = B;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            // The futures remain pinned in their boxes; the surrounding fields
            // are never structurally pinned.
            let this = unsafe { self.get_unchecked_mut() };
            if let Some(left) = &mut this.left
                && left.as_mut().poll(cx).is_ready()
            {
                this.left = None;
            }
            this.right.as_mut().poll(cx)
        }
    }
    Drive {
        left: Some(Box::pin(left)),
        right: Box::pin(right),
    }
    .await
}

/// Transfers an idiomatic input stream to the host's directional stdin
/// attachment. The host boundary is required for error-bearing byte streams,
/// which cannot rendezvous between sibling futures in one Component Model task.
pub fn pump_tool_stdin(source: InputStream) -> agentic_host_api::ToolStdin {
    agentic_host_api::create_stdin_from_stream(source)
}

type CachedInvocationResult = Result<InvocationResult, ToolError<TypedSchemaValue>>;
type InvocationResultDriver =
    crate::tool::invocation_result::InvocationResultDriver<CachedInvocationResult>;
type Completion<T> = Rc<dyn Fn() -> Pin<Box<dyn Future<Output = T>>>>;

/// The readable stdout of a started tool invocation.
///
/// Reading this stream also drives the invocation's shared result observer so
/// stdout-only consumers can make progress for filesystem-capable tools.
pub struct ToolInvocationStdout {
    stream: Option<InputStream>,
    result: Completion<()>,
}

impl ToolInvocationStdout {
    pub async fn next(&mut self) -> Option<Result<Vec<u8>, agentic_host_api::ByteStreamFailure>> {
        let stream = self.stream.as_mut()?;
        drive_left_until_right((self.result)(), stream.next()).await
    }

    pub async fn collect(mut self) -> Vec<Result<Vec<u8>, agentic_host_api::ByteStreamFailure>> {
        let mut output = Vec::new();
        while let Some(item) = self.next().await {
            output.push(item);
        }
        output
    }

    pub fn close(&mut self) {
        self.stream = None;
    }
}

/// A started stdout-bearing tool call. Output, structured completion, and
/// cancellation are independent capabilities.
pub struct ToolInvocation<T, E> {
    pub stdout: ToolInvocationStdout,
    future: Rc<agentic_host_api::FutureInvokeResult>,
    result: Completion<Result<T, ToolError<E>>>,
}

impl<T, E> ToolInvocation<T, E> {
    /// Returns an independently owned structured-completion future. The
    /// stdout field may be moved into a concurrent consumer after this call.
    pub fn result(&self) -> impl Future<Output = Result<T, ToolError<E>>> + use<T, E> {
        (self.result)()
    }

    pub fn cancel(&self) {
        self.future.cancel();
    }

    /// Drives stdout and structured completion concurrently.
    pub async fn collect(self) -> Result<(T, Vec<u8>), ToolError<E>> {
        let result = self.result();
        let mut stdout = self.stdout;
        let output = async {
            let mut bytes = Vec::new();
            loop {
                match stdout.next().await {
                    None => return Ok(bytes),
                    Some(Ok(chunk)) => bytes.extend(chunk),
                    Some(Err(reason)) => {
                        return Err(tool_protocol_error(format!(
                            "tool stdout failed: {reason:?}"
                        )));
                    }
                }
            }
        };
        let (result, output) = join(result, output).await;
        Ok((result?, output?))
    }
}

fn decode_wire_invocation_result<E>(
    result: host::InvocationResult,
) -> Result<InvocationResult, ToolError<E>> {
    let host::InvocationResult { result, stdout } = result;
    if stdout.is_some() {
        return Err(protocol_error(
            "tool result unexpectedly contained an embedded stdout stream".to_string(),
        ));
    }
    let result = result
        .map(crate::decode_typed_schema_value_owned)
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;
    Ok(InvocationResult { result })
}

/// Starts a stdout-bearing invocation with a generated structured-result decoder.
pub async fn start_tool_invocation<T: 'static, E: 'static>(
    rpc: &impl StartedToolRpcClient,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
    decode: impl Fn(InvocationResult) -> Result<T, ToolError<E>> + 'static,
    decode_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String> + 'static,
) -> Result<ToolInvocation<T, E>, ToolError<E>> {
    start_tool_invocation_with_stdout(rpc, command_path, input, stdin, true, decode, decode_error)
        .await
}

pub async fn start_tool_invocation_with_stdout<T: 'static, E: 'static>(
    rpc: &impl StartedToolRpcClient,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
    attach_stdout: bool,
    decode: impl Fn(InvocationResult) -> Result<T, ToolError<E>> + 'static,
    decode_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String> + 'static,
) -> Result<ToolInvocation<T, E>, ToolError<E>> {
    let input = crate::encode_typed_schema_value_async(input)
        .await
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let stdin = stdin.map(pump_tool_stdin);
    let (stdout_target, stdout) = if attach_stdout {
        let (target, stream) = agentic_host_api::create_stdout();
        (Some(target), Some(stream))
    } else {
        (None, None)
    };
    let future = rpc.async_invoke_and_await_tool(command_path, input, stdin, stdout_target);
    let future = Rc::new(future);
    let result = Rc::new(InvocationResultDriver::new({
        let future = Rc::clone(&future);
        move || {
            Box::pin(async move {
                let result = future.get().await.map_err(|error| {
                    map_rpc_error(error, &|_, _| Ok::<Option<TypedSchemaValue>, String>(None))
                })?;
                decode_wire_invocation_result(result)
            })
        }
    }));
    let drive = Rc::clone(&result);
    let decode = Rc::new(decode);
    let decode_error = Rc::new(decode_error);
    Ok(ToolInvocation {
        stdout: ToolInvocationStdout {
            stream: stdout,
            result: Rc::new(move || {
                let drive = Rc::clone(&drive);
                Box::pin(async move {
                    let _ = drive.wait().await;
                })
            }),
        },
        future,
        result: Rc::new(move || {
            let result = Rc::clone(&result);
            let decode = Rc::clone(&decode);
            let decode_error = Rc::clone(&decode_error);
            Box::pin(async move {
                match result.wait().await {
                    Ok(value) => decode(value),
                    Err(ToolError::Rpc(error)) => Err(ToolError::Rpc(error)),
                    Err(ToolError::RemoteTool(error)) => Err(ToolError::RemoteTool(error)),
                    Err(ToolError::MalformedRemoteOutput(error)) => {
                        Err(ToolError::MalformedRemoteOutput(error))
                    }
                    Err(ToolError::Tool(value)) => match decode_error(String::new(), value) {
                        Ok(Some(error)) => Err(ToolError::Tool(error)),
                        Ok(None) => {
                            Err(tool_protocol_error("custom tool error is missing its name"))
                        }
                        Err(message) => Err(tool_protocol_error(message)),
                    },
                    Err(ToolError::UnknownCustomError(error)) => {
                        match error
                            .payload()
                            .and_then(|payload| decode_error(error.name.clone(), payload.clone()))
                        {
                            Ok(Some(error)) => Err(ToolError::Tool(error)),
                            Ok(None) => Err(ToolError::UnknownCustomError(error)),
                            Err(message) => Err(tool_protocol_error(message)),
                        }
                    }
                }
            })
        }),
    })
}

/// Starts an invocation from an already encoded direct wire input.
#[doc(hidden)]
pub async fn start_tool_invocation_direct_input<T: 'static, E: 'static>(
    rpc: &impl StartedToolRpcClient,
    command_path: &[String],
    input: crate::schema::wit::wire::TypedSchemaValue,
    stdin: Option<InputStream>,
    decode: impl Fn(DirectInvocationResult) -> Result<T, ToolError<E>> + 'static,
    recognizes_error: fn(&str) -> bool,
    decode_error: impl Fn(
        &str,
        &mut crate::schema::wit::direct::WireReader,
        i32,
    ) -> Result<Option<E>, String>
    + 'static,
) -> Result<ToolInvocation<T, E>, ToolError<E>> {
    let stdin = stdin.map(pump_tool_stdin);
    let (stdout_target, stdout) = agentic_host_api::create_stdout();
    let future = rpc.async_invoke_and_await_tool(command_path, input, stdin, Some(stdout_target));
    let future = Rc::new(future);
    let result = Rc::new(crate::tool::invocation_result::InvocationResultDriver::new(
        {
            let future = Rc::clone(&future);
            move || {
                Box::pin(async move {
                    let result = future
                        .get()
                        .await
                        .map_err(|error| cache_direct_error(error, recognizes_error))?;
                    decode_direct_wire_invocation_result(result)
                })
            }
        },
    ));
    let drive = Rc::clone(&result);
    let decode = Rc::new(decode);
    let decode_error = Rc::new(decode_error);
    Ok(ToolInvocation {
        stdout: ToolInvocationStdout {
            stream: Some(stdout),
            result: Rc::new(move || {
                let drive = Rc::clone(&drive);
                Box::pin(async move {
                    let _ = drive.wait().await;
                })
            }),
        },
        future,
        result: Rc::new(move || {
            let result = Rc::clone(&result);
            let decode = Rc::clone(&decode);
            let decode_error = Rc::clone(&decode_error);
            Box::pin(async move {
                match result.wait().await {
                    Ok(value) => decode(value),
                    Err(error) => Err(decode_cached_direct_error(error, &*decode_error)),
                }
            })
        }),
    })
}

#[derive(Clone)]
struct DirectErrorPayload {
    name: String,
    value: DirectInvocationResult,
}

fn cache_direct_error(
    error: WitRpcError,
    recognizes: fn(&str) -> bool,
) -> ToolError<DirectErrorPayload> {
    match error {
        WitRpcError::RemoteToolError(WitToolError::CustomError(error))
            if recognizes(&error.name) =>
        {
            ToolError::Tool(DirectErrorPayload {
                name: error.name,
                value: DirectInvocationResult {
                    root: Some(error.payload.value.root),
                    snapshot: Rc::new(crate::schema::wit::direct::WireSnapshot::new(
                        error.payload.value.value_nodes,
                    )),
                },
            })
        }
        WitRpcError::RemoteToolError(WitToolError::CustomError(error)) => {
            ToolError::UnknownCustomError(RawCustomToolError::from_wire(error.name, error.payload))
        }
        other => map_infallible_rpc_error(other),
    }
}

fn decode_cached_direct_error<E>(
    error: ToolError<DirectErrorPayload>,
    decode: &impl Fn(
        &str,
        &mut crate::schema::wit::direct::WireReader,
        i32,
    ) -> Result<Option<E>, String>,
) -> ToolError<E> {
    match error {
        ToolError::Rpc(error) => ToolError::Rpc(error),
        ToolError::RemoteTool(error) => ToolError::RemoteTool(error),
        ToolError::MalformedRemoteOutput(error) => ToolError::MalformedRemoteOutput(error),
        ToolError::UnknownCustomError(error) => ToolError::UnknownCustomError(error),
        ToolError::Tool(error) => {
            let mut reader = error.value.snapshot.reader();
            match decode(&error.name, &mut reader, error.value.root.unwrap()) {
                Ok(Some(value)) => match reader.finish() {
                    Ok(()) => ToolError::Tool(value),
                    Err(error) => tool_protocol_error(error.to_string()),
                },
                Ok(None) => tool_protocol_error("declared custom tool error was not decoded"),
                Err(message) => tool_protocol_error(message),
            }
        }
    }
}

fn map_remote_tool_error<E>(
    error: WitToolError,
    decode_error: &(impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String> + ?Sized),
) -> ToolError<E> {
    match error {
        WitToolError::CustomError(error) => match decode_custom_tool_error_value(error.payload) {
            Ok(value) => match decode_error(error.name.clone(), value.clone()) {
                Ok(Some(error)) => ToolError::Tool(error),
                Ok(None) => ToolError::UnknownCustomError(RawCustomToolError::from_payload(
                    error.name, value,
                )),
                Err(message) => ToolError::Rpc(RpcError::Protocol(message)),
            },
            Err(message) => ToolError::Rpc(RpcError::Protocol(message)),
        },
        WitToolError::InvalidToolName(name) => {
            ToolError::RemoteTool(RemoteToolError::InvalidToolName(name))
        }
        WitToolError::InvalidCommandPath(path) => {
            ToolError::RemoteTool(RemoteToolError::InvalidCommandPath(path))
        }
        WitToolError::InvalidInput(message) => {
            ToolError::RemoteTool(RemoteToolError::InvalidInput(message))
        }
        WitToolError::ConstraintViolation(message) => {
            ToolError::RemoteTool(RemoteToolError::ConstraintViolation(message))
        }
        WitToolError::InvalidResult(message) => {
            ToolError::RemoteTool(RemoteToolError::InvalidResult(message))
        }
    }
}

fn decode_custom_tool_error<E: FromSchema>(
    _name: String,
    value: TypedSchemaValue,
) -> Result<Option<E>, String> {
    E::from_value(value.value())
        .map(Some)
        .map_err(format_from_schema_error)
}

fn decode_custom_tool_error_value(
    value: crate::schema::wit::wire::TypedSchemaValue,
) -> Result<TypedSchemaValue, String> {
    crate::decode_typed_schema_value_owned(value)
        .map_err(|error| format!("failed to decode remote tool error: {error}"))
}

fn format_from_schema_error(error: FromSchemaError) -> String {
    format!("failed to decode remote tool error: {error}")
}

fn protocol_error<E>(message: String) -> ToolError<E> {
    ToolError::Rpc(RpcError::Protocol(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::wit::wire::CustomToolError;
    use crate::{FromSchema, IntoSchema, IntoTypedSchemaValue};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    #[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
    enum CliError {
        Usage(String),
    }

    #[derive(
        Clone,
        Debug,
        Eq,
        PartialEq,
        IntoSchema,
        FromSchema,
        crate::IntoWire,
        crate::FromWire,
        crate::WireSchema,
    )]
    struct CallerPayload {
        message: String,
    }

    #[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
    struct RemotePayload {
        message: String,
    }

    #[derive(Clone, Debug, Eq, PartialEq, crate::ToolError)]
    enum DeclaredError {
        #[tool_error(kind = "usage-error", exit_code = 2)]
        Usage(CallerPayload),
    }

    #[test]
    fn generated_result_decoder_accepts_resolved_graph_equivalence() {
        let value = RemotePayload {
            message: "ok".to_string(),
        }
        .into_typed_schema_value()
        .unwrap();
        assert_eq!(
            decode_result_value::<CallerPayload, Infallible>(InvocationResult {
                result: Some(value),
            })
            .unwrap(),
            CallerPayload {
                message: "ok".to_string(),
            }
        );
    }

    #[test]
    fn generated_error_decoder_accepts_resolved_graph_equivalence() {
        let value = RemotePayload {
            message: "bad".to_string(),
        }
        .into_typed_schema_value()
        .unwrap();
        assert_eq!(
            decode_declared_tool_error::<DeclaredError>("usage".to_string(), value).unwrap(),
            Some(DeclaredError::Usage(CallerPayload {
                message: "bad".to_string(),
            }))
        );
    }

    #[test]
    async fn direct_completion_observers_decode_non_clone_values_without_models() {
        use crate::schema::wit::{direct, wire};
        struct ResultOnly(u32);
        impl direct::FromWire for ResultOnly {
            fn read_wire(
                reader: &mut direct::WireReader,
                index: i32,
            ) -> Result<Self, direct::WireError> {
                u32::read_wire(reader, index).map(Self)
            }
        }
        let count = Rc::new(Cell::new(0));
        let source = Rc::clone(&count);
        let driver = Rc::new(crate::tool::invocation_result::InvocationResultDriver::new(
            move || {
                source.set(source.get() + 1);
                Box::pin(async {
                    decode_direct_wire_invocation_result::<Infallible>(host::InvocationResult {
                        // The host has already validated the declared graph; concrete decoding
                        // checks the value's shape, not a newly reconstructed schema graph.
                        result: Some(wire::TypedSchemaValue {
                            graph: direct::schema::<String>(),
                            value: direct::encode(&83u32).unwrap(),
                        }),
                        stdout: None,
                    })
                })
            },
        ));
        let (first, second) = join(Rc::clone(&driver).wait(), Rc::clone(&driver).wait()).await;
        for result in [first, second, Rc::clone(&driver).wait().await] {
            assert_eq!(
                decode_direct_result_value::<ResultOnly, Infallible>(result.unwrap())
                    .unwrap()
                    .0,
                83
            );
        }
        assert_eq!(count.get(), 1);
    }

    #[test]
    async fn captured_input_preserves_names_aliases_and_affine_preflight() {
        let first = DirectInputValue::new("verbose", vec!["v".into()], None, true);
        let second =
            DirectInputValue::new("pattern", vec!["query".into()], None, "needle".to_string());
        let mut input = crate::agentic::DirectToolInput::new(
            encode_direct_tool_input(&[first, second], &["pattern", "verbose"])
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(input.take::<String>("query").unwrap(), "needle");
        assert!(input.take::<bool>("v").unwrap());
        input.finish().unwrap();

        type MaybeString = Option<String>;
        let aliased_option = DirectInputValue::new(
            "maybe",
            vec![],
            None,
            Some("present".to_string()) as MaybeString,
        )
        .with_option_carrier(true);
        let mut input = crate::agentic::DirectToolInput::new(
            encode_direct_tool_input(&[aliased_option], &["maybe"])
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            input.take::<MaybeString>("maybe").unwrap(),
            Some("present".to_string())
        );
        input.finish().unwrap();

        use crate::schema::wit::{GuestSecretHandle, wire};
        let handle = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(71) });
        let captured = DirectInputValue::new("secret", vec![], None, handle.clone());
        let alias = DirectInputValue::new("other", vec![], None, handle.clone());
        assert!(
            encode_direct_tool_input(&[captured, alias], &["secret", "other"])
                .await
                .is_err()
        );
        assert!(handle.is_present());
        assert_eq!(handle.take().unwrap().take_handle(), 71);
    }

    #[test]
    fn tool_rpc_errors_are_shared_with_oplog_bindings() {
        use crate::bindings::golem::api::oplog;

        let payload = "bad flag".to_string().into_typed_schema_value().unwrap();
        let error = WitRpcError::RemoteToolError(WitToolError::CustomError(CustomToolError {
            name: "usage".to_string(),
            payload: crate::encode_typed_schema_value(&payload).unwrap(),
        }));
        let error: host::ToolRpcError = error;
        let error: agentic_host_api::ToolRpcError = error;
        let recorded = oplog::ExternalToolResultParameters { result: Err(error) };
        let error = recorded.result.unwrap_err();
        let decoded = map_rpc_error(error, &|name, value| {
            assert_eq!(name, "usage");
            String::from_value(value.value())
                .map(CliError::Usage)
                .map(Some)
                .map_err(format_from_schema_error)
        });
        assert_eq!(
            decoded,
            ToolError::Tool(CliError::Usage("bad flag".to_string()))
        );

        let recorded = oplog::ToolInvocationResult {
            result: Some(crate::encode_typed_schema_value(&payload).unwrap()),
        };
        let value = crate::decode_typed_schema_value_owned(recorded.result.unwrap()).unwrap();
        assert_eq!(String::from_value(value.value()).unwrap(), "bad flag");
    }

    #[test]
    fn rpc_cancellation_and_resource_exhaustion_remain_distinct() {
        assert_eq!(
            map_infallible_rpc_error::<Infallible>(WitRpcError::Cancelled),
            ToolError::Rpc(RpcError::Cancelled)
        );
        assert_eq!(
            map_infallible_rpc_error::<Infallible>(WitRpcError::ResourceExhausted(
                "stdout limit".to_string()
            )),
            ToolError::Rpc(RpcError::ResourceExhausted("stdout limit".to_string()))
        );
    }

    #[test]
    fn custom_tool_error_payload_decodes_to_declared_error_variant() {
        let payload = "bad flag".to_string().into_typed_schema_value().unwrap();
        let wire_payload = crate::encode_typed_schema_value(&payload).unwrap();

        let decoded = map_remote_tool_error(
            WitToolError::CustomError(CustomToolError {
                name: "usage".to_string(),
                payload: wire_payload,
            }),
            &|name, value| {
                assert_eq!(name, "usage");
                String::from_value(value.value())
                    .map(CliError::Usage)
                    .map(Some)
                    .map_err(format_from_schema_error)
            },
        );

        assert_eq!(
            decoded,
            ToolError::Tool(CliError::Usage("bad flag".to_string()))
        );
    }

    #[test]
    fn direct_unknown_custom_error_defers_dynamic_payload_decoding() {
        use crate::schema::wit::{direct, wire};
        let cached = cache_direct_error(
            WitRpcError::RemoteToolError(WitToolError::CustomError(CustomToolError {
                name: "undeclared".to_string(),
                payload: wire::TypedSchemaValue {
                    graph: wire::SchemaGraph {
                        type_nodes: vec![],
                        defs: vec![],
                        root: -1,
                    },
                    value: direct::encode(&37u32).unwrap(),
                },
            })),
            |_| false,
        );
        let decoded: ToolError<Infallible> = decode_cached_direct_error(cached, &|_, _, _| {
            panic!("undeclared error must not enter the concrete decoder")
        });
        let ToolError::UnknownCustomError(error) = decoded else {
            panic!("unknown wire error was decoded eagerly")
        };
        assert_eq!(error.name, "undeclared");
        assert!(format!("{error:?}").contains("undeclared"));
        let other_observer = error.clone();
        assert!(error.payload().is_err());
        assert_eq!(
            error.payload().unwrap_err(),
            other_observer.payload().unwrap_err()
        );
    }

    #[test]
    fn unknown_custom_tool_error_preserves_name_and_owned_payload() {
        let payload = "raw".to_string().into_typed_schema_value().unwrap();
        let wire_payload = crate::encode_typed_schema_value(&payload).unwrap();
        let decoded: ToolError<CliError> = map_remote_tool_error(
            WitToolError::CustomError(CustomToolError {
                name: "new-error".to_string(),
                payload: wire_payload,
            }),
            &|_, _| Ok(None),
        );

        let ToolError::UnknownCustomError(error) = decoded else {
            panic!("unknown error was not preserved")
        };
        assert_eq!(error.name, "new-error");
        assert_eq!(
            String::from_value(error.payload().unwrap().value()).unwrap(),
            "raw".to_string()
        );
    }

    #[test]
    fn known_custom_tool_error_with_invalid_payload_is_protocol_error() {
        let payload = 42u32.into_typed_schema_value().unwrap();
        let wire_payload = crate::encode_typed_schema_value(&payload).unwrap();
        let decoded: ToolError<CliError> = map_remote_tool_error(
            WitToolError::CustomError(CustomToolError {
                name: "usage".to_string(),
                payload: wire_payload,
            }),
            &|name, value| {
                assert_eq!(name, "usage");
                String::from_value(value.value())
                    .map(CliError::Usage)
                    .map(Some)
                    .map_err(format_from_schema_error)
            },
        );
        assert!(matches!(decoded, ToolError::Rpc(RpcError::Protocol(_))));
    }

    #[test]
    async fn invocation_result_driver_shares_one_source_and_caches_its_outcome() {
        let starts = Rc::new(Cell::new(0));
        let polls = Rc::new(Cell::new(0));
        let driver = Rc::new(InvocationResultDriver::new({
            let starts = Rc::clone(&starts);
            let polls = Rc::clone(&polls);
            move || {
                starts.set(starts.get() + 1);
                Box::pin(poll_fn(move |cx| {
                    let poll_count = polls.get() + 1;
                    polls.set(poll_count);
                    if poll_count == 1 {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(ToolError::Rpc(RpcError::Cancelled)))
                    }
                }))
            }
        }));

        let (first, second) = join(Rc::clone(&driver).wait(), Rc::clone(&driver).wait()).await;
        let cached = Rc::clone(&driver).wait().await;

        for outcome in [first, second, cached] {
            assert!(matches!(outcome, Err(ToolError::Rpc(RpcError::Cancelled))));
        }
        assert_eq!(starts.get(), 1, "the host get future is created once");
        assert_eq!(
            polls.get(),
            2,
            "cached observers do not poll the host future"
        );
    }

    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn result_source_wakes_surviving_observer_when_latest_observer_is_dropped() {
        struct Source {
            ready: Cell<bool>,
            polls: Cell<usize>,
            waker: RefCell<Option<Waker>>,
        }

        let starts = Rc::new(Cell::new(0));
        let source = Rc::new(Source {
            ready: Cell::new(false),
            polls: Cell::new(0),
            waker: RefCell::new(None),
        });
        let driver = Rc::new(InvocationResultDriver::new({
            let starts = Rc::clone(&starts);
            let source = Rc::clone(&source);
            move || {
                starts.set(starts.get() + 1);
                Box::pin(poll_fn(move |cx| {
                    source.polls.set(source.polls.get() + 1);
                    if source.ready.get() {
                        Poll::Ready(Ok(InvocationResult { result: None }))
                    } else {
                        *source.waker.borrow_mut() = Some(cx.waker().clone());
                        Poll::Pending
                    }
                }))
            }
        }));

        let first_wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let second_wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let first_waker = Waker::from(Arc::clone(&first_wake));
        let second_waker = Waker::from(Arc::clone(&second_wake));
        let mut first_context = Context::from_waker(&first_waker);
        let mut second_context = Context::from_waker(&second_waker);
        let mut first = Box::pin(Rc::clone(&driver).wait());
        let mut second = Box::pin(Rc::clone(&driver).wait());

        assert!(first.as_mut().poll(&mut first_context).is_pending());
        assert!(second.as_mut().poll(&mut second_context).is_pending());
        drop(second);

        source.ready.set(true);
        source
            .waker
            .borrow_mut()
            .take()
            .expect("source registered the driver's stable waker")
            .wake();

        assert_eq!(first_wake.0.load(Ordering::SeqCst), 1);
        assert_eq!(second_wake.0.load(Ordering::SeqCst), 1);
        assert!(matches!(
            first.as_mut().poll(&mut first_context),
            Poll::Ready(Ok(InvocationResult { result: None }))
        ));
        assert_eq!(starts.get(), 1, "the host get future is created once");
        assert_eq!(source.polls.get(), 3);
    }

    struct FakeToolRpc;

    impl ToolRpcClient for FakeToolRpc {
        type Stdin = ();
        type Stdout = ();

        async fn invoke_and_await_tool(
            &self,
            _command_path: &[String],
            _input: crate::schema::wit::wire::TypedSchemaValue,
            _stdin: Option<Self::Stdin>,
            _stdout: Option<Self::Stdout>,
        ) -> Result<host::InvocationResult, WitRpcError> {
            let payload = "bad flag".to_string().into_typed_schema_value().unwrap();
            let wire_payload = crate::encode_typed_schema_value(&payload).unwrap();

            Err(WitRpcError::RemoteToolError(WitToolError::CustomError(
                CustomToolError {
                    name: "usage".to_string(),
                    payload: wire_payload,
                },
            )))
        }
    }

    enum FakeFailure {
        Denied,
        RemoteInvalidInput,
    }

    struct FailingToolRpc(FakeFailure);

    impl ToolRpcClient for FailingToolRpc {
        type Stdin = ();
        type Stdout = ();

        async fn invoke_and_await_tool(
            &self,
            _command_path: &[String],
            _input: crate::schema::wit::wire::TypedSchemaValue,
            _stdin: Option<Self::Stdin>,
            _stdout: Option<Self::Stdout>,
        ) -> Result<host::InvocationResult, WitRpcError> {
            Err(match self.0 {
                FakeFailure::Denied => WitRpcError::Denied("no access".to_string()),
                FakeFailure::RemoteInvalidInput => WitRpcError::RemoteToolError(
                    WitToolError::InvalidInput("bad wire input".to_string()),
                ),
            })
        }
    }

    #[test]
    async fn invoke_and_await_decoding_error_decodes_custom_tool_error_payload() {
        let input = ().into_typed_schema_value().unwrap();

        let decode_error = |name: String, value: TypedSchemaValue| {
            assert_eq!(name, "usage");
            String::from_value(value.value())
                .map(CliError::Usage)
                .map(Some)
                .map_err(format_from_schema_error)
        };

        match invoke_and_await(&FakeToolRpc, &[], &input, None, None, decode_error).await {
            Err(ToolError::Tool(CliError::Usage(message))) => assert_eq!(message, "bad flag"),
            Err(ToolError::Rpc(error)) => {
                panic!("expected declared tool error, got RPC error: {error:?}")
            }
            Err(ToolError::RemoteTool(error)) => {
                panic!("expected declared tool error, got remote tool error: {error:?}")
            }
            Err(ToolError::UnknownCustomError(error)) => {
                panic!("expected declared tool error, got unknown error: {error:?}")
            }
            Err(ToolError::MalformedRemoteOutput(message)) => {
                panic!("expected declared tool error, got malformed output: {message}")
            }
            Ok(_) => panic!("expected declared tool error, got success"),
        }
    }

    #[test]
    async fn invoke_and_await_distinguishes_rpc_and_remote_tool_errors() {
        let input = ().into_typed_schema_value().unwrap();

        match invoke_and_await_payload_error::<CliError, _>(
            &FailingToolRpc(FakeFailure::Denied),
            &[],
            &input,
            None,
            None,
        )
        .await
        {
            Err(ToolError::Rpc(RpcError::Denied(message))) => assert_eq!(message, "no access"),
            Err(other) => panic!("expected denied RPC error, got {other:?}"),
            Ok(_) => panic!("expected denied RPC error, got success"),
        }

        match invoke_and_await_payload_error::<CliError, _>(
            &FailingToolRpc(FakeFailure::RemoteInvalidInput),
            &[],
            &input,
            None,
            None,
        )
        .await
        {
            Err(ToolError::RemoteTool(RemoteToolError::InvalidInput(message))) => {
                assert_eq!(message, "bad wire input");
            }
            Err(other) => {
                panic!("expected structural remote tool error, got {other:?}")
            }
            Ok(_) => panic!("expected remote tool error, got success"),
        }
    }

    #[test]
    fn all_structural_remote_tool_errors_keep_their_variant() {
        let cases = [
            (
                WitToolError::InvalidToolName("bad name".to_string()),
                RemoteToolError::InvalidToolName("bad name".to_string()),
            ),
            (
                WitToolError::InvalidCommandPath(vec!["bad".to_string()]),
                RemoteToolError::InvalidCommandPath(vec!["bad".to_string()]),
            ),
            (
                WitToolError::InvalidInput("input".to_string()),
                RemoteToolError::InvalidInput("input".to_string()),
            ),
            (
                WitToolError::ConstraintViolation("constraint".to_string()),
                RemoteToolError::ConstraintViolation("constraint".to_string()),
            ),
            (
                WitToolError::InvalidResult("result".to_string()),
                RemoteToolError::InvalidResult("result".to_string()),
            ),
        ];
        for (wire, expected) in cases {
            let actual: ToolError<Infallible> =
                map_infallible_rpc_error(WitRpcError::RemoteToolError(wire));
            assert_eq!(actual, ToolError::RemoteTool(expected));
        }
    }

    #[test]
    async fn pending_result_observers_preserve_remote_tool_error() {
        let driver = Rc::new(InvocationResultDriver::new(|| {
            Box::pin(async {
                Err(map_rpc_error(
                    WitRpcError::RemoteToolError(WitToolError::ConstraintViolation(
                        "missing flag".to_string(),
                    )),
                    &|_, value| Ok(Some(value)),
                ))
            })
        }));
        let (first, second) = join(Rc::clone(&driver).wait(), Rc::clone(&driver).wait()).await;
        for outcome in [first, second] {
            assert!(matches!(
                outcome,
                Err(ToolError::RemoteTool(RemoteToolError::ConstraintViolation(message)))
                    if message == "missing flag"
            ));
        }
    }
}
