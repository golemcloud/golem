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

use std::cell::RefCell;
use std::convert::Infallible;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use crate::TypedSchemaValue;
use crate::agentic::AmbientToolRpc;
use crate::agentic::InputStream;
use crate::bindings::golem::tool::host::RpcError as WitRpcError;
use crate::bindings::golem::tool::host::{
    self, ToolRpc as HostToolRpc, ToolStdin as HostToolStdin, ToolStdout as HostToolStdout,
};
use crate::golem_agentic::golem::tool::host as agentic_host_api;
use crate::schema::{FromSchema, FromSchemaError};

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
#[derive(Clone)]
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
            .map_err(Into::into)
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
            .map_err(Into::into)
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
    let input = crate::encode_typed_schema_value(input)
        .map_err(|error| protocol_error(format!("failed to encode tool input: {error}")))?;
    let result = rpc
        .invoke_and_await_tool(command_path, input, stdin, stdout)
        .await
        .map_err(map_infallible_rpc_error)?;

    decode_wire_invocation_result(result)
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
            agentic_host::RpcError::RemoteToolError(error) => Self::RemoteToolError(error),
            agentic_host::RpcError::Cancelled => Self::Cancelled,
            agentic_host::RpcError::ResourceExhausted(message) => Self::ResourceExhausted(message),
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
type InvocationResultFuture = Pin<Box<dyn Future<Output = CachedInvocationResult>>>;
type InvocationResultFutureFactory = Box<dyn FnOnce() -> InvocationResultFuture>;

enum InvocationResultDriverState {
    Initial(Option<InvocationResultFutureFactory>),
    Polling(InvocationResultFuture),
    Ready(Box<CachedInvocationResult>),
}

#[derive(Default)]
struct InvocationResultWake {
    waiters: Mutex<Vec<Waker>>,
}

impl InvocationResultWake {
    fn register(&self, waker: &Waker) {
        let mut waiters = self.waiters.lock().expect("result waiters mutex poisoned");
        if !waiters.iter().any(|waiter| waiter.will_wake(waker)) {
            waiters.push(waker.clone());
        }
    }

    fn wake_waiters(&self) {
        let waiters =
            std::mem::take(&mut *self.waiters.lock().expect("result waiters mutex poisoned"));
        for waiter in waiters {
            waiter.wake();
        }
    }
}

impl Wake for InvocationResultWake {
    fn wake(self: Arc<Self>) {
        self.wake_waiters();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_waiters();
    }
}

struct InvocationResultDriver {
    state: RefCell<InvocationResultDriverState>,
    wake: Arc<InvocationResultWake>,
    source_waker: Waker,
}

impl InvocationResultDriver {
    fn new(factory: impl FnOnce() -> InvocationResultFuture + 'static) -> Self {
        let wake = Arc::new(InvocationResultWake::default());
        Self {
            state: RefCell::new(InvocationResultDriverState::Initial(Some(Box::new(
                factory,
            )))),
            source_waker: Waker::from(Arc::clone(&wake)),
            wake,
        }
    }

    fn poll(&self, cx: &mut Context<'_>) -> Poll<CachedInvocationResult> {
        loop {
            let mut state = self.state.borrow_mut();
            match &mut *state {
                InvocationResultDriverState::Initial(factory) => {
                    let future = factory
                        .take()
                        .expect("tool invocation result driver starts only once")(
                    );
                    *state = InvocationResultDriverState::Polling(future);
                }
                InvocationResultDriverState::Polling(future) => {
                    self.wake.register(cx.waker());
                    let mut source_context = Context::from_waker(&self.source_waker);
                    let Poll::Ready(result) = future.as_mut().poll(&mut source_context) else {
                        return Poll::Pending;
                    };
                    let result_for_caller = result.clone();
                    *state = InvocationResultDriverState::Ready(Box::new(result));
                    drop(state);
                    self.wake.wake_waiters();
                    return Poll::Ready(result_for_caller);
                }
                InvocationResultDriverState::Ready(result) => {
                    return Poll::Ready((**result).clone());
                }
            }
        }
    }

    async fn wait(self: Rc<Self>) -> CachedInvocationResult {
        poll_fn(|cx| self.poll(cx)).await
    }
}

/// The readable stdout of a started tool invocation.
///
/// Reading this stream also drives the invocation's shared result observer so
/// stdout-only consumers can make progress for filesystem-capable tools.
pub struct ToolInvocationStdout {
    stream: Option<InputStream>,
    result: Rc<InvocationResultDriver>,
}

impl ToolInvocationStdout {
    pub async fn next(&mut self) -> Option<Result<Vec<u8>, agentic_host_api::ByteStreamFailure>> {
        let stream = self.stream.as_mut()?;
        drive_left_until_right(Rc::clone(&self.result).wait(), stream.next()).await
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
    result: Rc<InvocationResultDriver>,
    decode: Rc<dyn Fn(InvocationResult) -> Result<T, ToolError<E>>>,
    decode_error: Rc<dyn Fn(TypedSchemaValue) -> Result<E, String>>,
}

impl<T, E> ToolInvocation<T, E> {
    /// Returns an independently owned structured-completion future. The
    /// stdout field may be moved into a concurrent consumer after this call.
    pub fn result(&self) -> impl Future<Output = Result<T, ToolError<E>>> + use<T, E> {
        let result = Rc::clone(&self.result);
        let decode = Rc::clone(&self.decode);
        let decode_error = Rc::clone(&self.decode_error);
        async move {
            match result.wait().await {
                Ok(result) => decode(result),
                Err(ToolError::Rpc(error)) => Err(ToolError::Rpc(error)),
                Err(ToolError::Tool(error)) => match decode_error(error) {
                    Ok(error) => Err(ToolError::Tool(error)),
                    Err(message) => Err(protocol_error(message)),
                },
            }
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
    result: host::InvocationResult,
) -> Result<InvocationResult, ToolError<E>> {
    let host::InvocationResult { result, stdout } = result;
    if stdout.is_some() {
        return Err(protocol_error(
            "tool result unexpectedly contained an embedded stdout stream".to_string(),
        ));
    }
    let result = result
        .map(|value| crate::decode_typed_schema_value(&value))
        .transpose()
        .map_err(|error| protocol_error(format!("failed to decode tool result: {error}")))?;
    Ok(InvocationResult { result })
}

/// Starts a stdout-bearing invocation with a generated structured-result decoder.
pub fn start_tool_invocation<T: 'static, E: 'static>(
    rpc: &impl StartedToolRpcClient,
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
    let future = rpc.async_invoke_and_await_tool(command_path, input, stdin, Some(stdout_target));
    let future = Rc::new(future);
    let result = Rc::new(InvocationResultDriver::new({
        let future = Rc::clone(&future);
        move || {
            Box::pin(async move {
                let result = future.get().await.map_err(|error| {
                    map_rpc_error(error.into(), &|value| Ok::<TypedSchemaValue, String>(value))
                })?;
                decode_wire_invocation_result(result)
            })
        }
    }));
    Ok(ToolInvocation {
        stdout: ToolInvocationStdout {
            stream: Some(stdout),
            result: Rc::clone(&result),
        },
        future,
        result,
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
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
