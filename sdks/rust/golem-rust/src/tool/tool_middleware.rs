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

use crate::bindings::golem::tool::common as wire;
use crate::schema::FromSchema;
use crate::schema::tool::{Doc, Tool};
use crate::{TypedSchemaValue, decode_typed_schema_value_owned, encode_typed_schema_value_owned};
use std::error::Error;
use std::fmt::{Display, Formatter};
#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;

/// Readable byte stream used for tool stdin and stdout.
pub type InputStream = wit_bindgen::StreamReader<u8>;

/// Successful result of a tool invocation.
pub struct InvocationResult {
    pub result: Option<TypedSchemaValue>,
    pub stdout: Option<InputStream>,
}

/// SDK-owned middleware metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolMiddleware {
    pub name: String,
    pub aliases: Vec<String>,
    pub doc: Doc,
    pub scope: ToolMiddlewareScope,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ToolMiddlewareScope {
    Monomorphic(MonomorphicToolMiddlewareScope),
    Universal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonomorphicToolMiddlewareScope {
    pub presented: Tool,
    pub expected: Option<Tool>,
}

/// Exact error channel shared by middleware guest dispatch and its underlying layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolInvokeError<E> {
    InvalidToolName(String),
    InvalidCommandPath(Vec<String>),
    InvalidInput(String),
    ConstraintViolation(String),
    InvalidResult(String),
    Tool(E),
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
/// cloneable, and mutable invocation prevents overlapping calls through safe Rust.
pub struct UnderlyingTool {
    inner: UnderlyingToolInner,
}

enum UnderlyingToolInner {
    Raw(wire::UnderlyingTool),
    #[cfg(test)]
    Fake(FakeInvoke),
}

#[cfg(test)]
type FakeInvoke = Box<
    dyn FnMut(
        Vec<String>,
        crate::schema::wit::wire::TypedSchemaValue,
        Option<InputStream>,
    ) -> Pin<
        Box<dyn Future<Output = Result<wire::InvocationResult, wire::ToolError>> + 'static>,
    >,
>;

impl UnderlyingTool {
    #[allow(dead_code)]
    pub(crate) fn from_raw(raw: wire::UnderlyingTool) -> Self {
        Self {
            inner: UnderlyingToolInner::Raw(raw),
        }
    }

    #[cfg(test)]
    fn from_fake(invoke: FakeInvoke) -> Self {
        Self {
            inner: UnderlyingToolInner::Fake(invoke),
        }
    }

    pub async fn invoke(
        &mut self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
    ) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
        self.invoke_with(command_path, input, stdin, Ok).await
    }

    #[doc(hidden)]
    pub async fn invoke_with<E>(
        &mut self,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        decode_custom_error: impl FnOnce(TypedSchemaValue) -> Result<E, String>,
    ) -> Result<InvocationResult, ToolInvokeError<E>> {
        let input = encode_typed_schema_value_owned(input)
            .map_err(|error| ToolInvokeError::InvalidInput(error.to_string()))?;
        let result = match &mut self.inner {
            UnderlyingToolInner::Raw(raw) => raw.invoke(command_path, input, stdin).await,
            #[cfg(test)]
            UnderlyingToolInner::Fake(invoke) => invoke(command_path, input, stdin).await,
        }
        .map_err(|error| decode_wire_error(error, decode_custom_error))?;
        decode_wire_result(result)
    }
}

fn decode_wire_result<E>(
    result: wire::InvocationResult,
) -> Result<InvocationResult, ToolInvokeError<E>> {
    let wire::InvocationResult { result, stdout } = result;
    let result = result
        .map(decode_typed_schema_value_owned)
        .transpose()
        .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
    Ok(InvocationResult { result, stdout })
}

fn decode_wire_error<E>(
    error: wire::ToolError,
    decode_custom_error: impl FnOnce(TypedSchemaValue) -> Result<E, String>,
) -> ToolInvokeError<E> {
    match error {
        wire::ToolError::InvalidToolName(name) => ToolInvokeError::InvalidToolName(name),
        wire::ToolError::InvalidCommandPath(path) => ToolInvokeError::InvalidCommandPath(path),
        wire::ToolError::InvalidInput(message) => ToolInvokeError::InvalidInput(message),
        wire::ToolError::ConstraintViolation(message) => {
            ToolInvokeError::ConstraintViolation(message)
        }
        wire::ToolError::InvalidResult(message) => ToolInvokeError::InvalidResult(message),
        wire::ToolError::CustomError(value) => {
            let value = match decode_typed_schema_value_owned(value) {
                Ok(value) => value,
                Err(error) => return ToolInvokeError::InvalidResult(error.to_string()),
            };
            match decode_custom_error(value) {
                Ok(error) => ToolInvokeError::Tool(error),
                Err(error) => ToolInvokeError::InvalidResult(error),
            }
        }
    }
}

pub fn decode_result_with_stdout<T: FromSchema, E>(
    result: InvocationResult,
) -> Result<(T, InputStream), ToolInvokeError<E>> {
    let stdout = expect_stdout(result.stdout)?;
    let value = decode_expected_value(result.result)?;
    Ok((value, stdout))
}

pub fn decode_result_value<T: FromSchema, E>(
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

fn decode_expected_value<T: FromSchema, E>(
    value: Option<TypedSchemaValue>,
) -> Result<T, ToolInvokeError<E>> {
    let value = value.ok_or_else(|| {
        ToolInvokeError::InvalidResult("tool result did not contain a value".to_string())
    })?;
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
            let decoded = decode_wire_error(variant, |_| -> Result<u32, String> { unreachable!() });
            match decoded {
                ToolInvokeError::InvalidToolName(value) => assert_eq!(value, "tool"),
                ToolInvokeError::InvalidCommandPath(value) => assert_eq!(value, ["sub"]),
                ToolInvokeError::InvalidInput(value) => assert_eq!(value, "input"),
                ToolInvokeError::ConstraintViolation(value) => assert_eq!(value, "constraint"),
                ToolInvokeError::InvalidResult(value) => assert_eq!(value, "result"),
                ToolInvokeError::Tool(_) => panic!("protocol error became a custom error"),
            }
        }
    }

    #[test]
    fn custom_error_is_decoded_and_decode_failure_is_invalid_result() {
        let payload = "failure".to_string().into_typed_schema_value().unwrap();
        let wire_payload = encode_typed_schema_value_owned(payload).unwrap();
        let decoded = decode_wire_error(wire::ToolError::CustomError(wire_payload), |value| {
            String::from_value(value.value()).map_err(|error| error.to_string())
        });
        assert_eq!(decoded, ToolInvokeError::Tool("failure".to_string()));

        let payload = "failure".to_string().into_typed_schema_value().unwrap();
        let wire_payload = encode_typed_schema_value_owned(payload).unwrap();
        let decoded =
            decode_wire_error::<String>(wire::ToolError::CustomError(wire_payload), |_| {
                Err("wrong custom payload".to_string())
            });
        assert_eq!(
            decoded,
            ToolInvokeError::InvalidResult("wrong custom payload".to_string())
        );
    }

    #[test]
    async fn one_mutable_handle_allows_sequential_owned_invocations() {
        let calls = Rc::new(Cell::new(0));
        let calls_for_fake = Rc::clone(&calls);
        let mut underlying = UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
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
