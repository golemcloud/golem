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
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use crate::TypedSchemaValue;
use crate::agentic::AmbientToolRpc;
use crate::agentic::InputStream;
use crate::bindings::golem::tool::host::RpcError as WitRpcError;
use crate::bindings::golem::tool::host::{
    self, ToolRpc as HostToolRpc, ToolStdin as HostToolStdin, ToolStdout as HostToolStdout,
};
use crate::golem_agentic::golem::tool::host as agentic_host_api;
use crate::schema::{FromSchema, FromSchemaError};

pub use crate::tool::InvocationResult;

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolError<E> {
    Rpc(RpcError),
    Tool(E),
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
        inherited_prefix: Vec<crate::agentic::CanonicalInputValue>,
    ) -> Self;
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
}

/// Decodes a structured invocation result and pairs it with its independently
/// acquired stdout stream.
pub fn decode_result_with_stdout<T: FromSchema, E>(
    result: InvocationResult,
    stdout: InputStream,
) -> Result<(T, InputStream), ToolError<E>> {
    let value = decode_expected_value(result.result)?;
    Ok((value, stdout))
}

/// Decodes an invocation result declared to carry a value.
pub fn decode_result_value<T: FromSchema, E>(result: InvocationResult) -> Result<T, ToolError<E>> {
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

fn decode_expected_value<T: FromSchema, E>(
    value: Option<TypedSchemaValue>,
) -> Result<T, ToolError<E>> {
    let value = expect_value(value)?;
    T::from_value(value.value()).map_err(|error| protocol_error(error.to_string()))
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
    async fn invoke_and_await_tool(
        &self,
        command_path: &[String],
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<host::InvocationResult, WitRpcError> {
        self.inner
            .invoke_and_await(command_path.to_vec(), input, stdin)
            .await
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
            .map_err(Into::into)
    }
}

/// Invokes a tool and decodes remote custom errors with a generated error decoder.
pub async fn invoke_and_await<E, R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
    decode_error: impl Fn(TypedSchemaValue) -> Result<E, String>,
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
    decode_error: impl Fn(TypedSchemaValue) -> Result<E, String>,
) -> Result<InvocationResult, ToolError<E>> {
    let input = crate::encode_typed_schema_value(input)
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(|error| map_rpc_error(error, &decode_error))?;

    let result_value = result
        .result
        .as_ref()
        .map(crate::decode_typed_schema_value)
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;

    Ok(InvocationResult {
        result: result_value,
    })
}

/// Invokes a zero-error tool and treats remote custom errors as protocol failures.
pub async fn invoke_and_await_infallible<R: ToolRpcClient>(
    rpc: &R,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<R::Stdin>,
    stdout: Option<R::Stdout>,
) -> Result<InvocationResult, ToolError<Infallible>> {
    let input = crate::encode_typed_schema_value(input)
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(map_infallible_rpc_error)?;

    let result_value = result
        .result
        .as_ref()
        .map(crate::decode_typed_schema_value)
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;

    Ok(InvocationResult {
        result: result_value,
    })
}

impl From<crate::golem_agentic::golem::tool::host::InvocationResult> for host::InvocationResult {
    fn from(result: crate::golem_agentic::golem::tool::host::InvocationResult) -> Self {
        Self {
            result: result.result,
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
            agentic_host::RpcError::Cancelled => Self::Cancelled,
            agentic_host::RpcError::ResourceExhausted(message) => Self::ResourceExhausted(message),
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
    decode_error: &(impl Fn(TypedSchemaValue) -> Result<E, String> + ?Sized),
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

/// Transfers an idiomatic input stream to the host's directional stdin
/// attachment. The host boundary is required for error-bearing byte streams,
/// which cannot rendezvous between sibling futures in one Component Model task.
pub fn pump_tool_stdin(source: InputStream) -> agentic_host_api::ToolStdin {
    agentic_host_api::create_stdin_from_stream(source)
}

/// A started stdout-bearing tool call. Output, structured completion, and
/// cancellation are independent capabilities.
pub struct ToolInvocation<T, E> {
    pub stdout: InputStream,
    future: Rc<agentic_host_api::FutureInvokeResult>,
    decode: Rc<dyn Fn(InvocationResult) -> Result<T, ToolError<E>>>,
    decode_error: Rc<dyn Fn(TypedSchemaValue) -> Result<E, String>>,
}

impl<T, E> ToolInvocation<T, E> {
    /// Returns an independently owned structured-completion future. The
    /// stdout field may be moved into a concurrent consumer after this call.
    pub fn result(&self) -> impl Future<Output = Result<T, ToolError<E>>> + use<T, E> {
        let future = Rc::clone(&self.future);
        let decode = Rc::clone(&self.decode);
        let decode_error = Rc::clone(&self.decode_error);
        async move {
            let result = future
                .get()
                .await
                .map_err(|error| map_rpc_error(error.into(), &*decode_error))?;
            let result = decode_wire_invocation_result(result)?;
            decode(result)
        }
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
    result: agentic_host_api::InvocationResult,
) -> Result<InvocationResult, ToolError<E>> {
    let result = result
        .result
        .map(|value| crate::decode_typed_schema_value(&value))
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;
    Ok(InvocationResult { result })
}

/// Starts a stdout-bearing invocation with a generated structured-result decoder.
pub fn start_tool_invocation<T: 'static, E: 'static>(
    rpc: &agentic_host_api::ToolRpc,
    command_path: &[String],
    input: &TypedSchemaValue,
    stdin: Option<InputStream>,
    decode: impl Fn(InvocationResult) -> Result<T, ToolError<E>> + 'static,
    decode_error: impl Fn(TypedSchemaValue) -> Result<E, String> + 'static,
) -> Result<ToolInvocation<T, E>, ToolError<E>> {
    let input = crate::encode_typed_schema_value(input)
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let stdin = stdin.map(pump_tool_stdin);
    let (stdout_target, stdout) = agentic_host_api::create_stdout();
    let future = rpc.async_invoke_and_await(command_path, input, stdin, Some(stdout_target));
    Ok(ToolInvocation {
        stdout,
        future: Rc::new(future),
        decode: Rc::new(decode),
        decode_error: Rc::new(decode_error),
    })
}

fn map_remote_tool_error<E>(
    error: host::ToolError,
    decode_error: &(impl Fn(TypedSchemaValue) -> Result<E, String> + ?Sized),
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
    fn rpc_cancellation_and_resource_exhaustion_remain_distinct() {
        assert_eq!(
            map_infallible_rpc_error(WitRpcError::Cancelled),
            ToolError::Rpc(RpcError::Cancelled)
        );
        assert_eq!(
            map_infallible_rpc_error(WitRpcError::ResourceExhausted("stdout limit".to_string())),
            ToolError::Rpc(RpcError::ResourceExhausted("stdout limit".to_string()))
        );
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

            Err(WitRpcError::RemoteToolError(host::ToolError::CustomError(
                wire_payload,
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
                    host::ToolError::InvalidInput("bad wire input".to_string()),
                ),
            })
        }
    }

    #[test]
    async fn invoke_and_await_decoding_error_decodes_custom_tool_error_payload() {
        let input = ().into_typed_schema_value().unwrap();

        let decode_error = |value: TypedSchemaValue| {
            String::from_value(value.value())
                .map(CliError::Usage)
                .map_err(format_from_schema_error)
        };

        match invoke_and_await(&FakeToolRpc, &[], &input, None, None, decode_error).await {
            Err(ToolError::Tool(CliError::Usage(message))) => assert_eq!(message, "bad flag"),
            Err(ToolError::Rpc(error)) => {
                panic!("expected declared tool error, got RPC error: {error:?}")
            }
            Ok(_) => panic!("expected declared tool error, got success"),
        }
    }

    #[test]
    async fn invoke_and_await_maps_framing_errors_to_rpc_errors() {
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
            Err(ToolError::Rpc(RpcError::Protocol(message))) => {
                assert!(
                    message.contains("remote tool error: invalid input: bad wire input"),
                    "unexpected protocol error message: {message}"
                );
            }
            Err(other) => {
                panic!("expected remote framing error to map to protocol RPC error, got {other:?}")
            }
            Ok(_) => {
                panic!("expected remote framing error to map to protocol RPC error, got success")
            }
        }
    }
}
