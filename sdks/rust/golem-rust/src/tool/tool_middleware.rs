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

#[cfg(any(test, feature = "export_golem_agentic"))]
use super::wire;
use crate::TypedSchemaValue;
#[cfg(any(test, feature = "export_golem_agentic"))]
use crate::decode_typed_schema_value_owned;
#[cfg(test)]
use crate::encode_typed_schema_value_owned;
use crate::schema::{FromSchema, IntoSchema};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

/// Readable byte stream used for tool stdin and stdout.
#[cfg(not(feature = "export_golem_agentic"))]
pub type InputStream = wit_bindgen::StreamReader<
    Result<Vec<u8>, crate::bindings::golem::tool::streams::ByteStreamFailure>,
>;

/// Writable byte stream supplied to middleware for tool stdout.
#[cfg(feature = "export_golem_agentic")]
pub type OutputStream = crate::agentic::OutputStream;
#[cfg(not(feature = "export_golem_agentic"))]
#[doc(hidden)]
pub struct OutputStream;
#[cfg(feature = "export_golem_agentic")]
pub type InputStream = wit_bindgen::StreamReader<
    Result<Vec<u8>, crate::golem_agentic::golem::tool::streams::ByteStreamFailure>,
>;

/// Successful result of a tool invocation.
pub struct InvocationResult {
    pub result: Option<TypedSchemaValue>,
    pub stdout: Option<InputStream>,
}

#[doc(hidden)]
pub type ToolMiddlewareInvokeFutureFor<'a> = Pin<
    Box<dyn Future<Output = Result<InvocationResult, ToolInvokeError<RawCustomToolError>>> + 'a>,
>;

#[doc(hidden)]
pub type ToolMiddlewareInvokeFuture = ToolMiddlewareInvokeFutureFor<'static>;

/// A custom tool error whose payload is decoded only on explicit inspection.
#[derive(Clone)]
pub struct RawCustomToolError {
    pub name: String,
    payload: Rc<RawCustomToolPayload>,
}

struct RawCustomToolPayload {
    wire: std::cell::RefCell<Option<crate::schema::wit::wire::TypedSchemaValue>>,
    decoded: std::cell::OnceCell<Result<TypedSchemaValue, String>>,
}

impl RawCustomToolError {
    pub fn from_payload(name: String, payload: TypedSchemaValue) -> Self {
        Self {
            name,
            payload: Rc::new(RawCustomToolPayload {
                wire: std::cell::RefCell::new(None),
                decoded: std::cell::OnceCell::from(Ok(payload)),
            }),
        }
    }

    pub fn from_wire(name: String, payload: crate::schema::wit::wire::TypedSchemaValue) -> Self {
        Self {
            name,
            payload: Rc::new(RawCustomToolPayload {
                wire: std::cell::RefCell::new(Some(payload)),
                decoded: std::cell::OnceCell::new(),
            }),
        }
    }

    /// Materializes the dynamic schema model for an undeclared error.
    pub fn payload(&self) -> Result<&TypedSchemaValue, String> {
        self.payload
            .decoded
            .get_or_init(|| {
                crate::decode_typed_schema_value_owned(
                    self.payload
                        .wire
                        .borrow_mut()
                        .take()
                        .expect("undecoded payload"),
                )
                .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl std::fmt::Debug for RawCustomToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawCustomToolError")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RawCustomToolError {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.payload() == other.payload()
    }
}

/// Exact error channel shared by middleware guest dispatch and its underlying layer.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ToolInvokeError<E> {
    InvalidToolName(String),
    InvalidCommandPath(Vec<String>),
    InvalidInput(String),
    ConstraintViolation(String),
    InvalidResult(String),
    Tool(E),
    UnknownCustomError(RawCustomToolError),
    ProtocolError(String),
    Denied(String),
    InternalError(String),
    Cancelled,
    ResourceExhausted(String),
}

impl<E> ToolInvokeError<E> {
    /// Transforms only the tool-defined custom error payload.
    pub fn map_tool<F, O>(self, transform: F) -> ToolInvokeError<O>
    where
        F: FnOnce(E) -> O,
    {
        match self {
            Self::InvalidToolName(name) => ToolInvokeError::InvalidToolName(name),
            Self::InvalidCommandPath(path) => ToolInvokeError::InvalidCommandPath(path),
            Self::InvalidInput(message) => ToolInvokeError::InvalidInput(message),
            Self::ConstraintViolation(message) => ToolInvokeError::ConstraintViolation(message),
            Self::InvalidResult(message) => ToolInvokeError::InvalidResult(message),
            Self::Tool(error) => ToolInvokeError::Tool(transform(error)),
            Self::UnknownCustomError(error) => ToolInvokeError::UnknownCustomError(error),
            Self::ProtocolError(message) => ToolInvokeError::ProtocolError(message),
            Self::Denied(message) => ToolInvokeError::Denied(message),
            Self::InternalError(message) => ToolInvokeError::InternalError(message),
            Self::Cancelled => ToolInvokeError::Cancelled,
            Self::ResourceExhausted(message) => ToolInvokeError::ResourceExhausted(message),
        }
    }
}

impl<E> From<E> for ToolInvokeError<E> {
    fn from(error: E) -> Self {
        Self::Tool(error)
    }
}

impl<E: Display> Display for ToolInvokeError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidToolName(name) => write!(f, "invalid tool name `{name}`"),
            Self::InvalidCommandPath(path) => {
                write!(f, "invalid command path `{}`", path.join(" "))
            }
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::ConstraintViolation(message) => write!(f, "constraint violation: {message}"),
            Self::InvalidResult(message) => write!(f, "invalid result: {message}"),
            Self::Tool(error) => error.fmt(f),
            Self::UnknownCustomError(error) => {
                write!(f, "unknown custom tool error `{}`", error.name)
            }
            Self::ProtocolError(message) => write!(f, "protocol error: {message}"),
            Self::Denied(message) => write!(f, "denied: {message}"),
            Self::InternalError(message) => write!(f, "internal error: {message}"),
            Self::Cancelled => write!(f, "underlying tool invocation was cancelled"),
            Self::ResourceExhausted(message) => write!(f, "resource exhausted: {message}"),
        }
    }
}

impl<E: Error + 'static> Error for ToolInvokeError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Tool(error) => Some(error),
            _ => None,
        }
    }
}

/// Invocation-scoped access to exactly the next inner tool-middleware layer.
///
/// The runtime is the only producer of this handle. It is intentionally not
/// cloneable. Shared invocation permits a middleware to overlap calls to the
/// same runtime-minted capability.
#[cfg_attr(not(any(test, feature = "export_golem_agentic")), allow(dead_code))]
pub struct UnderlyingTool {
    inner: UnderlyingToolInner,
}

/// One admitted invocation of the next middleware layer.
///
/// Dropping this value only stops observing the invocation. Use [`Self::cancel`]
/// to explicitly request cancellation of this child.
#[cfg_attr(not(any(test, feature = "export_golem_agentic")), allow(dead_code))]
pub struct UnderlyingInvocation {
    result: Rc<UnderlyingInvocationResult>,
    #[cfg(any(test, feature = "export_golem_agentic"))]
    terminal: Rc<
        super::invocation_result::InvocationResultDriver<
            Result<Option<TypedSchemaValue>, ToolInvokeError<std::convert::Infallible>>,
        >,
    >,
    pub stdout: Option<InputStream>,
}

/// Typed view of a started underlying invocation generated for a tool command.
#[cfg_attr(not(any(test, feature = "export_golem_agentic")), allow(dead_code))]
pub struct TypedUnderlyingInvocation<T, E> {
    invocation: UnderlyingInvocation,
    pub stdout: Option<InputStream>,
    decode: fn(InvocationResult) -> Result<T, ToolInvokeError<E>>,
    decode_error: fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
}

#[cfg(any(test, feature = "export_golem_agentic"))]
impl<T, E> TypedUnderlyingInvocation<T, E> {
    #[doc(hidden)]
    pub fn new(
        mut invocation: UnderlyingInvocation,
        decode: fn(InvocationResult) -> Result<T, ToolInvokeError<E>>,
        decode_error: fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
    ) -> Self {
        let stdout = invocation.stdout.take();
        Self {
            invocation,
            stdout,
            decode,
            decode_error,
        }
    }

    pub fn cancel(&self) {
        self.invocation.cancel();
    }

    pub async fn get(&self) -> Result<T, ToolInvokeError<E>> {
        let result = self.invocation.get_with(self.decode_error).await?;
        (self.decode)(InvocationResult {
            result,
            stdout: None,
        })
    }

    /// Waits for the structured result while forwarding the underlying stdout
    /// to the writer supplied to this middleware invocation.
    #[cfg(feature = "export_golem_agentic")]
    pub async fn get_forwarding_stdout(
        mut self,
        stdout: Option<OutputStream>,
    ) -> Result<T, ToolInvokeError<E>> {
        let underlying_stdout = self.stdout.take();
        let forwarded = async move {
            if underlying_stdout.is_none() {
                return Err(ToolInvokeError::InvalidResult(
                    "tool result did not contain declared stdout stream".to_string(),
                ));
            }
            forward_stdout(underlying_stdout, stdout).await
        };
        let result = self.get();
        let (result, forwarded) = join_results(result, forwarded).await;
        match result {
            Err(error) => Err(error),
            Ok(result) => {
                forwarded?;
                Ok(result)
            }
        }
    }
}

enum UnderlyingInvocationResult {
    #[cfg(feature = "export_golem_agentic")]
    Raw(crate::tool_underlying_bindings::UnderlyingInvokeResult),
    #[cfg(test)]
    Fake(FakeInvocationResult),
}

#[cfg(test)]
type FakeInvocationResultFuture = Pin<
    Box<
        dyn Future<
            Output = Result<Option<crate::schema::wit::wire::TypedSchemaValue>, wire::ToolError>,
        >,
    >,
>;

#[cfg(test)]
struct FakeInvocationResult {
    result: std::cell::RefCell<Option<FakeInvocationResultFuture>>,
    cancelled: Rc<std::cell::Cell<bool>>,
}

#[cfg(any(test, feature = "export_golem_agentic"))]
impl UnderlyingInvocation {
    fn new(result: UnderlyingInvocationResult, stdout: Option<InputStream>) -> Self {
        let result = Rc::new(result);
        let source = Rc::clone(&result);
        let terminal = Rc::new(super::invocation_result::InvocationResultDriver::new(
            move || {
                Box::pin(async move {
                    let result = match source.as_ref() {
                        #[cfg(feature = "export_golem_agentic")]
                        UnderlyingInvocationResult::Raw(result) => result
                            .get()
                            .await
                            .map_err(|error| decode_underlying_error(error, |_, _| Ok(None)))?,
                        #[cfg(test)]
                        UnderlyingInvocationResult::Fake(result) => {
                            let future = result
                                .result
                                .borrow_mut()
                                .take()
                                .expect("observer starts once");
                            future
                                .await
                                .map_err(|error| decode_wire_error(error, |_, _| Ok(None)))?
                        }
                    };
                    result
                        .map(decode_typed_schema_value_owned)
                        .transpose()
                        .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))
                })
            },
        ));
        Self {
            result,
            terminal,
            stdout,
        }
    }

    pub fn cancel(&self) {
        match self.result.as_ref() {
            #[cfg(feature = "export_golem_agentic")]
            UnderlyingInvocationResult::Raw(result) => result.cancel(),
            #[cfg(test)]
            UnderlyingInvocationResult::Fake(result) => result.cancelled.set(true),
        }
    }

    pub async fn get(
        &self,
    ) -> Result<Option<TypedSchemaValue>, ToolInvokeError<RawCustomToolError>> {
        self.get_with(|name, payload| Ok(Some(RawCustomToolError::from_payload(name, payload))))
            .await
    }

    #[doc(hidden)]
    pub async fn get_with<E>(
        &self,
        decode_custom_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
    ) -> Result<Option<TypedSchemaValue>, ToolInvokeError<E>> {
        Rc::clone(&self.terminal)
            .wait()
            .await
            .map_err(|error| match error {
                ToolInvokeError::UnknownCustomError(raw) => {
                    match raw
                        .payload()
                        .and_then(|payload| decode_custom_error(raw.name.clone(), payload.clone()))
                    {
                        Ok(Some(value)) => ToolInvokeError::Tool(value),
                        Ok(None) => ToolInvokeError::UnknownCustomError(raw),
                        Err(error) => ToolInvokeError::InvalidResult(error),
                    }
                }
                other => other.map_tool(|impossible| match impossible {}),
            })
    }
}

enum UnderlyingToolInner {
    #[cfg(feature = "export_golem_agentic")]
    Raw(crate::tool_underlying_bindings::UnderlyingTool),
    #[cfg(test)]
    Fake(FakeInvoke),
}

#[cfg(test)]
type FakeRawInvoke = Box<
    dyn Fn(
        Vec<String>,
        crate::schema::wit::wire::TypedSchemaValue,
        Option<InputStream>,
    ) -> Pin<Box<dyn Future<Output = Result<wire::InvocationResult, wire::ToolError>>>>,
>;

#[cfg(test)]
pub(crate) type FakeInvoke = Box<
    dyn Fn(
        Vec<String>,
        crate::schema::wit::wire::TypedSchemaValue,
        Option<InputStream>,
    ) -> (
        Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Option<crate::schema::wit::wire::TypedSchemaValue>,
                            wire::ToolError,
                        >,
                    > + 'static,
            >,
        >,
        Option<InputStream>,
        Rc<std::cell::Cell<bool>>,
    ),
>;

#[cfg(any(test, feature = "export_golem_agentic"))]
impl UnderlyingTool {
    #[cfg(feature = "export_golem_agentic")]
    #[allow(dead_code)]
    pub(crate) fn from_raw(raw: crate::tool_underlying_bindings::UnderlyingTool) -> Self {
        Self {
            inner: UnderlyingToolInner::Raw(raw),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_fake(invoke: FakeRawInvoke) -> Self {
        let invoke = Box::new(move |path, input, stdin| {
            let result = invoke(path, input, stdin);
            let result = Box::pin(async move {
                let result = result.await?;
                if result.stdout.is_some() {
                    return Err(wire::ToolError::InvalidResult(
                        "fake invocation must provide stdout when it is started".to_string(),
                    ));
                }
                Ok(result.result)
            });
            (result as _, None, Rc::new(std::cell::Cell::new(false)))
        });
        Self {
            inner: UnderlyingToolInner::Fake(invoke),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_fake_started(invoke: FakeInvoke) -> Self {
        Self {
            inner: UnderlyingToolInner::Fake(invoke),
        }
    }

    pub async fn invoke(
        &self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
        self.invoke_with(command_path, input, stdin, |name, payload| {
            Ok(Some(RawCustomToolError::from_payload(name, payload)))
        })
        .await
    }

    /// Invokes the next layer while forwarding its stdout to the writer
    /// supplied to this middleware invocation.
    #[cfg(feature = "export_golem_agentic")]
    pub async fn invoke_forwarding_stdout(
        &self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        stdout: Option<OutputStream>,
    ) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
        let mut invocation = self.start(command_path, input, stdin).await?;
        let forwarded = forward_stdout(invocation.stdout.take(), stdout);
        let result = invocation.get();
        let (result, forwarded) = join_results(result, forwarded).await;
        match result {
            Err(error) => Err(error),
            Ok(result) => {
                forwarded?;
                Ok(InvocationResult {
                    result,
                    stdout: None,
                })
            }
        }
    }

    /// Admits a call and returns its independently cancellable result observer
    /// before waiting for the structured result.
    pub async fn start(
        &self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<UnderlyingInvocation, ToolInvokeError<RawCustomToolError>> {
        self.start_with(command_path, input, stdin).await
    }

    #[doc(hidden)]
    pub async fn start_with<E>(
        &self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<UnderlyingInvocation, ToolInvokeError<E>> {
        let input = crate::encode_typed_schema_value_async(&input)
            .await
            .map_err(|error| ToolInvokeError::InvalidInput(error.to_string()))?;
        match &self.inner {
            #[cfg(feature = "export_golem_agentic")]
            UnderlyingToolInner::Raw(raw) => {
                let (result, stdout) = raw.invoke(command_path, input, stdin).await;
                Ok(UnderlyingInvocation::new(
                    UnderlyingInvocationResult::Raw(result),
                    stdout,
                ))
            }
            #[cfg(test)]
            UnderlyingToolInner::Fake(invoke) => {
                let (result, stdout, cancelled) = invoke(command_path, input, stdin);
                Ok(UnderlyingInvocation::new(
                    UnderlyingInvocationResult::Fake(FakeInvocationResult {
                        result: std::cell::RefCell::new(Some(result)),
                        cancelled,
                    }),
                    stdout,
                ))
            }
        }
    }

    #[doc(hidden)]
    pub async fn invoke_with<E>(
        &self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        decode_custom_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
    ) -> Result<InvocationResult, ToolInvokeError<E>> {
        let mut invocation = self.start_with(command_path, input, stdin).await?;
        let result = invocation.get_with(decode_custom_error).await?;
        Ok(InvocationResult {
            result,
            stdout: invocation.stdout.take(),
        })
    }
}

#[cfg(feature = "export_golem_agentic")]
async fn forward_stdout<E>(
    stdout: Option<InputStream>,
    mut output: Option<OutputStream>,
) -> Result<(), ToolInvokeError<E>> {
    let Some(mut stdout) = stdout else {
        return match output {
            Some(output) => classify_stream_write_result(output.finish().await).map(drop),
            None => Ok(()),
        };
    };
    while let Some(item) = stdout.next().await {
        match item {
            Ok(bytes) => {
                if let Some(output) = &mut output
                    && classify_stream_write_result(output.write(bytes).await)?
                {
                    return Ok(());
                }
            }
            Err(reason) => {
                return match output {
                    Some(output) => {
                        classify_stream_write_result(output.fail(reason).await).map(drop)
                    }
                    None => Ok(()),
                };
            }
        }
    }
    match output {
        Some(output) => classify_stream_write_result(output.finish().await).map(drop),
        None => Ok(()),
    }
}

#[cfg(feature = "export_golem_agentic")]
fn classify_stream_write_result<E>(
    result: Result<(), crate::golem_agentic::golem::tool::streams::StreamWriteError>,
) -> Result<bool, ToolInvokeError<E>> {
    use crate::golem_agentic::golem::tool::streams::{ByteStreamCloseCause, StreamWriteError};
    match result {
        Ok(()) => Ok(false),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::ConsumerCancelled)) => Ok(true),
        Err(error) => Err(ToolInvokeError::InvalidResult(format!(
            "failed to forward underlying stdout: {error:?}"
        ))),
    }
}

#[cfg(any(test, feature = "export_golem_agentic"))]
async fn join_results<A, B>(a: A, b: B) -> (A::Output, B::Output)
where
    A: Future,
    B: Future,
{
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    let mut a_result = None;
    let mut b_result = None;
    std::future::poll_fn(|cx| {
        if a_result.is_none()
            && let std::task::Poll::Ready(result) = a.as_mut().poll(cx)
        {
            a_result = Some(result);
        }
        if b_result.is_none()
            && let std::task::Poll::Ready(result) = b.as_mut().poll(cx)
        {
            b_result = Some(result);
        }
        match (a_result.take(), b_result.take()) {
            (Some(a), Some(b)) => std::task::Poll::Ready((a, b)),
            (a, b) => {
                a_result = a;
                b_result = b;
                std::task::Poll::Pending
            }
        }
    })
    .await
}

#[cfg(feature = "export_golem_agentic")]
fn decode_underlying_error<E>(
    error: crate::tool_underlying_bindings::UnderlyingError,
    decode_custom_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
) -> ToolInvokeError<E> {
    use crate::tool_underlying_bindings::UnderlyingError;
    match error {
        UnderlyingError::ToolError(error) => decode_wire_error(error, decode_custom_error),
        UnderlyingError::ProtocolError(message) => ToolInvokeError::ProtocolError(message),
        UnderlyingError::Denied(message) => ToolInvokeError::Denied(message),
        UnderlyingError::InternalError(message) => ToolInvokeError::InternalError(message),
        UnderlyingError::Cancelled => ToolInvokeError::Cancelled,
        UnderlyingError::ResourceExhausted(message) => ToolInvokeError::ResourceExhausted(message),
    }
}

#[cfg(any(test, feature = "export_golem_agentic"))]
fn decode_wire_error<E>(
    error: wire::ToolError,
    decode_custom_error: impl Fn(String, TypedSchemaValue) -> Result<Option<E>, String>,
) -> ToolInvokeError<E> {
    match error {
        wire::ToolError::InvalidToolName(name) => ToolInvokeError::InvalidToolName(name),
        wire::ToolError::InvalidCommandPath(path) => ToolInvokeError::InvalidCommandPath(path),
        wire::ToolError::InvalidInput(message) => ToolInvokeError::InvalidInput(message),
        wire::ToolError::ConstraintViolation(message) => {
            ToolInvokeError::ConstraintViolation(message)
        }
        wire::ToolError::InvalidResult(message) => ToolInvokeError::InvalidResult(message),
        wire::ToolError::CustomError(error) => {
            let value = match decode_typed_schema_value_owned(error.payload) {
                Ok(value) => value,
                Err(error) => return ToolInvokeError::InvalidResult(error.to_string()),
            };
            match decode_custom_error(error.name.clone(), value.clone()) {
                Ok(Some(error)) => ToolInvokeError::Tool(error),
                Ok(None) => ToolInvokeError::UnknownCustomError(RawCustomToolError::from_payload(
                    error.name, value,
                )),
                Err(error) => ToolInvokeError::InvalidResult(error),
            }
        }
    }
}

pub fn decode_result_with_stdout<T: FromSchema + IntoSchema, E>(
    result: InvocationResult,
) -> Result<(T, InputStream), ToolInvokeError<E>> {
    let stdout = expect_stdout(result.stdout)?;
    let value = decode_expected_value(result.result)?;
    Ok((value, stdout))
}

pub fn decode_result_value<T: FromSchema + IntoSchema, E>(
    result: InvocationResult,
) -> Result<T, ToolInvokeError<E>> {
    expect_no_stdout(result.stdout)?;
    decode_expected_value(result.result)
}

pub fn decode_result_stdout_only<E>(
    result: InvocationResult,
) -> Result<InputStream, ToolInvokeError<E>> {
    let stdout = expect_stdout(result.stdout)?;
    expect_no_value(result.result)?;
    Ok(stdout)
}

pub fn decode_result_empty<E>(result: InvocationResult) -> Result<(), ToolInvokeError<E>> {
    expect_no_stdout(result.stdout)?;
    expect_no_value(result.result)
}

fn decode_expected_value<T: FromSchema + IntoSchema, E>(
    value: Option<TypedSchemaValue>,
) -> Result<T, ToolInvokeError<E>> {
    let value = value.ok_or_else(|| {
        ToolInvokeError::InvalidResult("tool result did not contain a value".to_string())
    })?;
    let expected = crate::schema::try_into_schema_graph::<T>()
        .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
    if value.graph() != &expected {
        return Err(ToolInvokeError::InvalidResult(
            "tool result schema does not match the expected result schema".to_string(),
        ));
    }
    T::from_value(value.value()).map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))
}

fn expect_stdout<E>(stdout: Option<InputStream>) -> Result<InputStream, ToolInvokeError<E>> {
    stdout.ok_or_else(|| {
        ToolInvokeError::InvalidResult(
            "tool result did not contain declared stdout stream".to_string(),
        )
    })
}

fn expect_no_stdout<E>(stdout: Option<InputStream>) -> Result<(), ToolInvokeError<E>> {
    if stdout.is_some() {
        return Err(ToolInvokeError::InvalidResult(
            "tool result unexpectedly contained stdout stream".to_string(),
        ));
    }
    Ok(())
}

fn expect_no_value<E>(value: Option<TypedSchemaValue>) -> Result<(), ToolInvokeError<E>> {
    if value.is_some() {
        return Err(ToolInvokeError::InvalidResult(
            "tool result unexpectedly contained a value".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IntoTypedSchemaValue;
    use std::cell::Cell;
    use std::rc::Rc;
    use test_r::test;

    #[cfg(feature = "export_golem_agentic")]
    #[test]
    fn stdout_write_result_classification_only_accepts_consumer_cancellation() {
        use crate::golem_agentic::golem::tool::streams::{
            ByteStreamCloseCause, ByteStreamFailure, StreamWriteError,
        };

        assert_eq!(classify_stream_write_result::<()>(Ok(())), Ok(false));
        assert_eq!(
            classify_stream_write_result::<()>(Err(StreamWriteError::Closed(
                ByteStreamCloseCause::ConsumerCancelled
            ))),
            Ok(true)
        );
        for error in [
            StreamWriteError::Closed(ByteStreamCloseCause::Finished),
            StreamWriteError::Closed(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned)),
            StreamWriteError::ConcurrentOperation,
        ] {
            assert!(matches!(
                classify_stream_write_result::<()>(Err(error)),
                Err(ToolInvokeError::InvalidResult(_))
            ));
        }
    }

    #[test]
    fn map_tool_preserves_every_protocol_variant() {
        let variants = [
            ToolInvokeError::InvalidToolName("tool".to_string()),
            ToolInvokeError::InvalidCommandPath(vec!["sub".to_string()]),
            ToolInvokeError::InvalidInput("input".to_string()),
            ToolInvokeError::ConstraintViolation("constraint".to_string()),
            ToolInvokeError::InvalidResult("result".to_string()),
        ];

        for variant in variants {
            let mapped: ToolInvokeError<u64> = variant.clone().map_tool(|_: u32| unreachable!());
            assert_eq!(mapped.map_tool(|value| value as u32), variant);
        }
        assert_eq!(
            ToolInvokeError::Tool(41u32).map_tool(|value| value + 1),
            ToolInvokeError::Tool(42u32)
        );
    }

    #[test]
    fn wire_protocol_errors_are_preserved_exactly() {
        let variants = [
            wire::ToolError::InvalidToolName("tool".to_string()),
            wire::ToolError::InvalidCommandPath(vec!["sub".to_string()]),
            wire::ToolError::InvalidInput("input".to_string()),
            wire::ToolError::ConstraintViolation("constraint".to_string()),
            wire::ToolError::InvalidResult("result".to_string()),
        ];

        for variant in variants {
            let decoded = decode_wire_error(variant, |_, _| -> Result<Option<u32>, String> {
                unreachable!()
            });
            match decoded {
                ToolInvokeError::InvalidToolName(value) => assert_eq!(value, "tool"),
                ToolInvokeError::InvalidCommandPath(value) => assert_eq!(value, ["sub"]),
                ToolInvokeError::InvalidInput(value) => assert_eq!(value, "input"),
                ToolInvokeError::ConstraintViolation(value) => assert_eq!(value, "constraint"),
                ToolInvokeError::InvalidResult(value) => assert_eq!(value, "result"),
                ToolInvokeError::Tool(_) => panic!("protocol error became a custom error"),
                ToolInvokeError::UnknownCustomError(_) => {
                    panic!("protocol error became an unknown custom error")
                }
                ToolInvokeError::ProtocolError(_)
                | ToolInvokeError::Denied(_)
                | ToolInvokeError::InternalError(_)
                | ToolInvokeError::Cancelled
                | ToolInvokeError::ResourceExhausted(_) => {
                    panic!("wire protocol error became an underlying lifecycle error")
                }
            }
        }
    }

    #[test]
    fn custom_error_is_decoded_and_decode_failure_is_invalid_result() {
        let payload = "failure".to_string().into_typed_schema_value().unwrap();
        let wire_payload = encode_typed_schema_value_owned(payload).unwrap();
        let decoded = decode_wire_error(
            wire::ToolError::CustomError(crate::schema::wit::wire::CustomToolError {
                name: "failure".to_string(),
                payload: wire_payload,
            }),
            |name, value| {
                assert_eq!(name, "failure");
                String::from_value(value.value())
                    .map(Some)
                    .map_err(|error| error.to_string())
            },
        );
        assert_eq!(decoded, ToolInvokeError::Tool("failure".to_string()));

        let payload = "failure".to_string().into_typed_schema_value().unwrap();
        let wire_payload = encode_typed_schema_value_owned(payload).unwrap();
        let decoded = decode_wire_error::<String>(
            wire::ToolError::CustomError(crate::schema::wit::wire::CustomToolError {
                name: "failure".to_string(),
                payload: wire_payload,
            }),
            |_, _| Err("wrong custom payload".to_string()),
        );
        assert_eq!(
            decoded,
            ToolInvokeError::InvalidResult("wrong custom payload".to_string())
        );
    }

    #[test]
    async fn one_handle_allows_sequential_owned_invocations() {
        let calls = Rc::new(Cell::new(0));
        let calls_for_fake = Rc::clone(&calls);
        let underlying = UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
            assert!(stdin.is_none());
            assert_eq!(path, ["run"]);
            calls_for_fake.set(calls_for_fake.get() + 1);
            Box::pin(async move {
                Ok(wire::InvocationResult {
                    result: Some(input),
                    stdout: None,
                })
            })
        }));

        for value in ["first", "second"] {
            let result = underlying
                .invoke(
                    vec!["run".to_string()],
                    value.to_string().into_typed_schema_value().unwrap(),
                    None,
                )
                .await
                .unwrap();
            let decoded = String::from_value(result.result.unwrap().value()).unwrap();
            assert_eq!(decoded, value);
        }
        assert_eq!(calls.get(), 2);
    }

    #[test]
    async fn one_shared_handle_allows_overlapping_invocations_to_complete_in_reverse_order() {
        let admitted = Rc::new(Cell::new(0));
        let release_first = Rc::new(Cell::new(false));
        let admitted_for_fake = Rc::clone(&admitted);
        let release_first_for_fake = Rc::clone(&release_first);
        let underlying = UnderlyingTool::from_fake(Box::new(move |_, input, _| {
            admitted_for_fake.set(admitted_for_fake.get() + 1);
            let admitted = Rc::clone(&admitted_for_fake);
            let release_first = Rc::clone(&release_first_for_fake);
            Box::pin(async move {
                let value = decode_typed_schema_value_owned(input).unwrap();
                let value = String::from_value(value.value()).unwrap();
                if value == "first" {
                    std::future::poll_fn(|cx| {
                        if release_first.get() {
                            std::task::Poll::Ready(())
                        } else {
                            cx.waker().wake_by_ref();
                            std::task::Poll::Pending
                        }
                    })
                    .await;
                } else {
                    assert_eq!(admitted.get(), 2);
                    release_first.set(true);
                }
                Ok(wire::InvocationResult {
                    result: Some(
                        encode_typed_schema_value_owned(value.into_typed_schema_value().unwrap())
                            .unwrap(),
                    ),
                    stdout: None,
                })
            })
        }));

        let first = underlying.invoke(
            vec!["run".to_string()],
            "first".to_string().into_typed_schema_value().unwrap(),
            None,
        );
        let second = underlying.invoke(
            vec!["run".to_string()],
            "second".to_string().into_typed_schema_value().unwrap(),
            None,
        );
        let mut first = std::pin::pin!(first);
        let mut second = std::pin::pin!(second);
        let mut first_result = None;
        let mut second_result = None;
        let (first, second) = std::future::poll_fn(|cx| {
            if first_result.is_none()
                && let std::task::Poll::Ready(result) = first.as_mut().poll(cx)
            {
                first_result = Some(result);
            }
            if second_result.is_none()
                && let std::task::Poll::Ready(result) = second.as_mut().poll(cx)
            {
                assert!(first_result.is_none());
                second_result = Some(result);
            }
            match (first_result.take(), second_result.take()) {
                (Some(first), Some(second)) => std::task::Poll::Ready((first, second)),
                (first, second) => {
                    first_result = first;
                    second_result = second;
                    std::task::Poll::Pending
                }
            }
        })
        .await;
        assert_eq!(admitted.get(), 2);
        assert_eq!(
            String::from_value(second.unwrap().result.unwrap().value()).unwrap(),
            "second"
        );
        assert_eq!(
            String::from_value(first.unwrap().result.unwrap().value()).unwrap(),
            "first"
        );
    }

    #[test]
    async fn underlying_observer_shares_concurrent_gets_and_caches_terminal() {
        let polls = Rc::new(Cell::new(0));
        let underlying = UnderlyingTool::from_fake_started(Box::new({
            let polls = Rc::clone(&polls);
            move |_, input, _| {
                let polls = Rc::clone(&polls);
                let mut value = Some(input);
                let future = Box::pin(std::future::poll_fn(move |cx| {
                    polls.set(polls.get() + 1);
                    if polls.get() == 1 {
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(Ok(value.take()))
                    }
                }));
                (future as _, None, Rc::new(Cell::new(false)))
            }
        }));
        let expected = "shared terminal"
            .to_string()
            .into_typed_schema_value()
            .unwrap();
        let invocation = underlying
            .start(vec![], expected.clone(), None)
            .await
            .unwrap();
        assert_eq!(polls.get(), 0);
        let (first, second) = join_results(invocation.get(), invocation.get()).await;
        assert_eq!(first.unwrap(), Some(expected.clone()));
        assert_eq!(second.unwrap(), Some(expected.clone()));
        assert_eq!(invocation.get().await.unwrap(), Some(expected));
        assert_eq!(polls.get(), 2);
    }

    #[test]
    async fn result_wait_and_cancel_share_the_same_invocation_borrow() {
        let underlying = UnderlyingTool::from_fake_started(Box::new(|_, _, _| {
            let cancelled = Rc::new(Cell::new(false));
            let observed = Rc::clone(&cancelled);
            let result = Box::pin(std::future::poll_fn(move |cx| {
                if observed.get() {
                    std::task::Poll::Ready(Err(wire::ToolError::ConstraintViolation(
                        "cancelled".to_string(),
                    )))
                } else {
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            }));
            (result as _, None, cancelled)
        }));
        let invocation = underlying
            .start(
                vec!["run".to_string()],
                ().into_typed_schema_value().unwrap(),
                None,
            )
            .await
            .unwrap();
        let get = invocation.get();
        let mut get = std::pin::pin!(get);
        std::future::poll_fn(|cx| {
            assert!(get.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        invocation.cancel();
        assert!(matches!(
            get.await,
            Err(ToolInvokeError::ConstraintViolation(message)) if message == "cancelled"
        ));
        assert!(matches!(
            invocation.get().await,
            Err(ToolInvokeError::ConstraintViolation(message)) if message == "cancelled"
        ));
    }

    #[test]
    fn result_slot_projection_rejects_missing_and_unexpected_values() {
        let missing = decode_result_value::<String, ()>(InvocationResult {
            result: None,
            stdout: None,
        });
        assert!(matches!(missing, Err(ToolInvokeError::InvalidResult(_))));

        let unexpected = decode_result_empty::<()>(InvocationResult {
            result: Some("value".to_string().into_typed_schema_value().unwrap()),
            stdout: None,
        });
        assert!(matches!(unexpected, Err(ToolInvokeError::InvalidResult(_))));
    }

    #[test]
    fn result_slot_projection_rejects_missing_stream() {
        let missing = decode_result_stdout_only::<()>(InvocationResult {
            result: None,
            stdout: None,
        });
        assert!(matches!(missing, Err(ToolInvokeError::InvalidResult(_))));
    }
}
