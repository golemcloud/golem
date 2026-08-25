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

use super::super::{get_tool_middleware_by_name, get_tool_middleware_invoker_by_name};
use crate::agentic::ToolErrorSchema;
use crate::schema::wit::{GuestQuotaTokenHandle, GuestSecretHandle, wire as schema_wire};
use crate::schema::{FromSchema, IntoSchema, SchemaValue, TypedSchemaValue};
use crate::tool::wire;
use crate::tool::{
    InputStream, InvocationResult, Principal, Tool, ToolInvokeError, ToolMiddlewareScope,
    ToolUnderlying, UnderlyingTool,
};
use crate::{
    IntoTypedSchemaValue, decode_typed_schema_value_owned, encode_typed_schema_value_owned,
};
use golem_rust_macro::{ToolError, tool_definition, tool_middleware, universal_tool_middleware};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::future::Future;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use test_r::test;
use wit_bindgen::rt::async_support::StreamVtable;

static TEST_STREAMS: Mutex<BTreeMap<u32, VecDeque<u8>>> = Mutex::new(BTreeMap::new());
static NEXT_TEST_STREAM: AtomicU32 = AtomicU32::new(1);
static TEST_STREAM_NEW_CALLS: AtomicU32 = AtomicU32::new(0);

struct RegistryTestState;

#[test_r::test_dep(scope = Shared)]
fn registry_test_state() -> RegistryTestState {
    RegistryTestState
}

fn test_streams() -> MutexGuard<'static, BTreeMap<u32, VecDeque<u8>>> {
    TEST_STREAMS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn run_acceptance<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("host-independent acceptance future unexpectedly blocked"),
    }
}

unsafe extern "C" fn test_stream_new() -> u64 {
    TEST_STREAM_NEW_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

unsafe extern "C" fn test_stream_read(handle: u32, destination: *mut u8, amount: usize) -> u32 {
    let mut streams = test_streams();
    let Some(stream) = streams.get_mut(&handle) else {
        return 1;
    };
    let count = amount.min(stream.len());
    for offset in 0..count {
        unsafe {
            ptr::write(destination.add(offset), stream.pop_front().unwrap());
        }
    }
    ((count as u32) << 4) | u32::from(stream.is_empty())
}

unsafe extern "C" fn test_stream_write(_handle: u32, _source: *const u8, _amount: usize) -> u32 {
    1
}

unsafe extern "C" fn test_stream_cancel(_handle: u32) -> u32 {
    2
}

unsafe extern "C" fn test_stream_drop_readable(handle: u32) {
    test_streams().remove(&handle);
}

unsafe extern "C" fn test_stream_drop_writable(_handle: u32) {}

static TEST_STREAM_VTABLE: StreamVtable<u8> = StreamVtable {
    layout: std::alloc::Layout::new::<u8>(),
    lower: None,
    dealloc_lists: None,
    lift: None,
    start_write: test_stream_write,
    start_read: test_stream_read,
    cancel_write: test_stream_cancel,
    cancel_read: test_stream_cancel,
    drop_writable: test_stream_drop_writable,
    drop_readable: test_stream_drop_readable,
    new: test_stream_new,
};

fn canonical_input<T: ToolUnderlying>(
    command_path: &[&str],
    fields: Vec<SchemaValue>,
) -> TypedSchemaValue {
    let tool = T::__golem_tool_descriptor();
    let path = command_path
        .iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    let command_index = tool
        .command_index_by_path(&path)
        .expect("acceptance command path resolves");
    let model = tool
        .canonical_input_model(command_index)
        .expect("acceptance canonical input model builds");
    TypedSchemaValue::new(model.record_schema, SchemaValue::Record { fields })
}

fn typed_result(value: impl IntoSchema) -> wire::InvocationResult {
    wire::InvocationResult {
        result: Some(
            encode_typed_schema_value_owned(value.into_typed_schema_value().unwrap()).unwrap(),
        ),
        stdout: None,
    }
}

fn readable_stream(bytes: &[u8]) -> InputStream {
    let handle = NEXT_TEST_STREAM.fetch_add(1, Ordering::Relaxed);
    assert!(
        test_streams()
            .insert(handle, bytes.iter().copied().collect())
            .is_none()
    );
    wit_bindgen::StreamReader::new(handle, &TEST_STREAM_VTABLE)
}

async fn invoke(
    middleware_name: &str,
    tool_name: &str,
    tool: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    get_tool_middleware_invoker_by_name(middleware_name)
        .unwrap_or_else(|| panic!("middleware `{middleware_name}` is registered"))(
        tool_name.to_string(),
        tool,
        command_path,
        input,
        stdin,
        principal,
        underlying,
    )
    .await
}

fn invocation_error(
    result: Result<InvocationResult, ToolInvokeError<TypedSchemaValue>>,
) -> ToolInvokeError<TypedSchemaValue> {
    match result {
        Ok(_) => panic!("invocation unexpectedly succeeded"),
        Err(error) => error,
    }
}

#[tool_definition]
trait AcceptanceEcho {
    fn echo(&self, value: String) -> String;
}

struct AcceptancePolicy;

impl AcceptancePolicy {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "phase-six-transparent-policy",
    constructor = AcceptancePolicy::new
)]
impl AcceptanceEchoMiddleware for AcceptancePolicy {
    async fn echo(
        &self,
        underlying: &mut AcceptanceEchoUnderlying,
        value: String,
    ) -> Result<String, ToolInvokeError<Infallible>> {
        match value.as_str() {
            "reject" => Err(ToolInvokeError::ConstraintViolation(
                "rejected by middleware".to_string(),
            )),
            "short" => Ok("short-circuited".to_string()),
            "retry" => match underlying.echo(value.clone()).await {
                Ok(result) => Ok(result),
                Err(_) => underlying.echo(value).await,
            },
            "transform" => Ok(format!("outer({})", underlying.echo(value).await?)),
            _ => underlying.echo(value).await,
        }
    }
}

fn echo_underlying(calls: Rc<Cell<u32>>, fail_first_retry: bool) -> UnderlyingTool {
    UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
        assert_eq!(path, ["echo"]);
        assert!(stdin.is_none());
        let input = decode_typed_schema_value_owned(input).unwrap();
        let SchemaValue::Record { fields } = input.value() else {
            panic!("echo input is a record")
        };
        let value = String::from_value(&fields[0]).unwrap();
        let call = calls.get() + 1;
        calls.set(call);
        Box::pin(async move {
            if fail_first_retry && value == "retry" && call == 1 {
                Err(wire::ToolError::ConstraintViolation(
                    "transient".to_string(),
                ))
            } else {
                Ok(typed_result(format!("inner:{value}")))
            }
        })
    }))
}

async fn invoke_echo(
    value: &str,
    stdin: Option<InputStream>,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    invoke(
        "phase-six-transparent-policy",
        "acceptance-echo",
        AcceptanceEchoUnderlying::__golem_tool_descriptor(),
        vec!["echo".to_string()],
        canonical_input::<AcceptanceEchoUnderlying>(
            &["echo"],
            vec![SchemaValue::String(value.to_string())],
        ),
        stdin,
        Principal::Anonymous,
        underlying,
    )
    .await
}

#[test]
fn transparent_middleware_rejects_forwards_retries_short_circuits_and_transforms(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let calls = Rc::new(Cell::new(0));
        let rejected = invoke_echo("reject", None, echo_underlying(Rc::clone(&calls), false)).await;
        assert_eq!(
            invocation_error(rejected),
            ToolInvokeError::ConstraintViolation("rejected by middleware".to_string())
        );
        assert_eq!(calls.get(), 0);

        let short = invoke_echo("short", None, echo_underlying(Rc::clone(&calls), false))
            .await
            .unwrap();
        assert_eq!(
            String::from_value(short.result.unwrap().value()).unwrap(),
            "short-circuited"
        );
        assert_eq!(calls.get(), 0);

        let forwarded = invoke_echo("forward", None, echo_underlying(Rc::clone(&calls), false))
            .await
            .unwrap();
        assert_eq!(
            String::from_value(forwarded.result.unwrap().value()).unwrap(),
            "inner:forward"
        );
        assert_eq!(calls.get(), 1);

        calls.set(0);
        let retried = invoke_echo("retry", None, echo_underlying(Rc::clone(&calls), true))
            .await
            .unwrap();
        assert_eq!(
            String::from_value(retried.result.unwrap().value()).unwrap(),
            "inner:retry"
        );
        assert_eq!(calls.get(), 2);

        calls.set(0);
        let transformed = invoke_echo("transform", None, echo_underlying(Rc::clone(&calls), false))
            .await
            .unwrap();
        assert_eq!(
            String::from_value(transformed.result.unwrap().value()).unwrap(),
            "outer(inner:transform)"
        );
        assert_eq!(calls.get(), 1);
    });
}

fn protocol_error_underlying(error: wire::ToolError) -> UnderlyingTool {
    let error = Rc::new(RefCell::new(Some(error)));
    UnderlyingTool::from_fake(Box::new(move |_, _, _| {
        let error = error.borrow_mut().take().expect("one protocol call");
        Box::pin(async move { Err(error) })
    }))
}

#[test]
fn transparent_dispatch_preserves_all_five_protocol_errors_exactly(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let cases = [
            wire::ToolError::InvalidToolName("wrong-tool".to_string()),
            wire::ToolError::InvalidCommandPath(vec!["wrong".to_string(), "path".to_string()]),
            wire::ToolError::InvalidInput("bad-input".to_string()),
            wire::ToolError::ConstraintViolation("constraint".to_string()),
            wire::ToolError::InvalidResult("bad-result".to_string()),
        ];

        for case in cases {
            let error = invocation_error(
                invoke_echo("forward", None, protocol_error_underlying(case)).await,
            );
            match error {
                ToolInvokeError::InvalidToolName(value) => assert_eq!(value, "wrong-tool"),
                ToolInvokeError::InvalidCommandPath(value) => {
                    assert_eq!(value, ["wrong", "path"])
                }
                ToolInvokeError::InvalidInput(value) => assert_eq!(value, "bad-input"),
                ToolInvokeError::ConstraintViolation(value) => assert_eq!(value, "constraint"),
                ToolInvokeError::InvalidResult(value) => assert_eq!(value, "bad-result"),
                ToolInvokeError::Tool(_) => panic!("protocol error became a custom error"),
            }
        }
    });
}

#[derive(Debug, Eq, PartialEq, ToolError)]
enum PresentedError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Rejected(String),
}

#[derive(Debug, Eq, PartialEq, ToolError)]
enum BackendError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Failed(String),
}

#[tool_definition]
trait AdapterPresented {
    fn convert(&self, value: u32) -> Result<String, PresentedError>;
}

#[tool_definition]
trait AdapterBackend {
    fn execute(&self, encoded: String) -> Result<u64, BackendError>;
}

struct AdapterPolicy;

impl AdapterPolicy {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "phase-six-adapter-policy",
    constructor = AdapterPolicy::new
)]
impl AdapterPresentedMiddleware<AdapterBackendUnderlying> for AdapterPolicy {
    async fn convert(
        &self,
        underlying: &mut AdapterBackendUnderlying,
        value: u32,
    ) -> Result<String, ToolInvokeError<PresentedError>> {
        underlying
            .execute(format!("backend:{value}"))
            .await
            .map(|value| format!("public:{value}"))
            .map_err(|error| {
                error.map_tool(|BackendError::Failed(message)| PresentedError::Rejected(message))
            })
    }
}

type AdapterCalls = Rc<RefCell<Vec<(Vec<String>, String)>>>;

fn adapter_underlying(calls: AdapterCalls, result: Result<u64, String>) -> UnderlyingTool {
    UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
        assert!(stdin.is_none());
        let input = decode_typed_schema_value_owned(input).unwrap();
        let SchemaValue::Record { fields } = input.value() else {
            panic!("adapter input is a record")
        };
        let encoded = String::from_value(&fields[0]).unwrap();
        calls.borrow_mut().push((path, encoded));
        let result = result.clone();
        Box::pin(async move {
            match result {
                Ok(value) => Ok(typed_result(value)),
                Err(message) => Err(wire::ToolError::CustomError(
                    encode_typed_schema_value_owned(message.into_typed_schema_value().unwrap())
                        .unwrap(),
                )),
            }
        })
    }))
}

async fn invoke_adapter(
    value: u32,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    invoke(
        "phase-six-adapter-policy",
        "adapter-presented",
        AdapterPresentedUnderlying::__golem_tool_descriptor(),
        vec!["convert".to_string()],
        canonical_input::<AdapterPresentedUnderlying>(&["convert"], vec![SchemaValue::U32(value)]),
        None,
        Principal::Anonymous,
        underlying,
    )
    .await
}

#[test]
fn adapter_converts_input_output_and_custom_errors_between_exact_descriptors(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let descriptor = get_tool_middleware_by_name("phase-six-adapter-policy")
            .expect("adapter descriptor is registered");
        let ToolMiddlewareScope::Monomorphic(scope) = descriptor.scope else {
            panic!("adapter is monomorphic")
        };
        assert_eq!(
            scope.presented,
            AdapterPresentedUnderlying::__golem_tool_descriptor()
        );
        assert_eq!(
            scope.expected,
            Some(AdapterBackendUnderlying::__golem_tool_descriptor())
        );

        let calls = Rc::new(RefCell::new(Vec::new()));
        let result = invoke_adapter(42, adapter_underlying(Rc::clone(&calls), Ok(7)))
            .await
            .unwrap();
        assert_eq!(
            String::from_value(result.result.unwrap().value()).unwrap(),
            "public:7"
        );
        assert_eq!(
            calls.borrow().as_slice(),
            &[(vec!["execute".to_string()], "backend:42".to_string())]
        );

        let error = invocation_error(
            invoke_adapter(
                42,
                adapter_underlying(Rc::new(RefCell::new(Vec::new())), Err("denied".to_string())),
            )
            .await,
        );
        let ToolInvokeError::Tool(error) = error else {
            panic!("mapped adapter error is custom")
        };
        assert_eq!(
            PresentedError::from_error_payload_value(error).unwrap(),
            PresentedError::Rejected("denied".to_string())
        );
    });
}

#[tool_definition]
trait NestedLeaf {
    #[command(name = "leaf", aliases = ["l"])]
    fn leaf(&self, format: u32, name: String) -> String;
}

struct NestedLeafSubtree;

#[tool_definition]
trait NestedPresented {
    #[arg(count = "global", aliases = ["format"])]
    #[command(name = "branch", aliases = ["b"], subtree = NestedLeaf)]
    fn branch(&self, count: u32) -> NestedLeafSubtree;
}

struct NestedTransparent;

impl NestedTransparent {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "phase-six-nested-transparent",
    constructor = NestedTransparent::new
)]
impl NestedPresentedMiddleware for NestedTransparent {
    async fn branch__leaf(
        &self,
        underlying: &mut NestedPresentedUnderlying,
        count: u32,
        name: String,
    ) -> Result<String, ToolInvokeError<Infallible>> {
        underlying.branch__leaf(count, name).await
    }
}

struct NestedAdapter;

impl NestedAdapter {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "phase-six-nested-adapter",
    constructor = NestedAdapter::new
)]
impl NestedPresentedMiddleware<AdapterBackendUnderlying> for NestedAdapter {
    async fn branch__leaf(
        &self,
        underlying: &mut AdapterBackendUnderlying,
        count: u32,
        name: String,
    ) -> Result<String, ToolInvokeError<Infallible>> {
        underlying
            .execute(format!("{count}:{name}"))
            .await
            .map(|value| format!("adapted:{value}"))
            .map_err(|error| error.map_tool(|_| unreachable!()))
    }
}

type NestedCalls = Rc<RefCell<Vec<(Vec<String>, u32, String)>>>;

fn nested_transparent_underlying(calls: NestedCalls) -> UnderlyingTool {
    UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
        assert!(stdin.is_none());
        let input = decode_typed_schema_value_owned(input).unwrap();
        let SchemaValue::Record { fields } = input.value() else {
            panic!("nested input is a record")
        };
        let count = u32::from_value(&fields[0]).unwrap();
        let name = String::from_value(&fields[1]).unwrap();
        calls.borrow_mut().push((path, count, name.clone()));
        Box::pin(async move { Ok(typed_result(format!("nested:{count}:{name}"))) })
    }))
}

async fn invoke_nested(
    middleware_name: &str,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    invoke(
        middleware_name,
        "nested-presented",
        NestedPresentedUnderlying::__golem_tool_descriptor(),
        vec!["b".to_string(), "l".to_string()],
        canonical_input::<NestedPresentedUnderlying>(
            &["b", "l"],
            vec![
                SchemaValue::U32(9),
                SchemaValue::String("alice".to_string()),
            ],
        ),
        None,
        Principal::Anonymous,
        underlying,
    )
    .await
}

#[test]
fn nested_transparent_and_adapter_dispatch_honor_aliases_and_inherited_globals(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let result = invoke_nested(
            "phase-six-nested-transparent",
            nested_transparent_underlying(Rc::clone(&calls)),
        )
        .await
        .unwrap();
        assert_eq!(
            String::from_value(result.result.unwrap().value()).unwrap(),
            "nested:9:alice"
        );
        assert_eq!(
            calls.borrow().as_slice(),
            &[(
                vec!["branch".to_string(), "leaf".to_string()],
                9,
                "alice".to_string()
            )]
        );

        let adapter_calls = Rc::new(RefCell::new(Vec::new()));
        let result = invoke_nested(
            "phase-six-nested-adapter",
            adapter_underlying(Rc::clone(&adapter_calls), Ok(11)),
        )
        .await
        .unwrap();
        assert_eq!(
            String::from_value(result.result.unwrap().value()).unwrap(),
            "adapted:11"
        );
        assert_eq!(
            adapter_calls.borrow().as_slice(),
            &[(vec!["execute".to_string()], "9:alice".to_string())]
        );
    });
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UniversalObservation {
    tool_name: String,
    tool_version: String,
    command_path: Vec<String>,
    anonymous_principal: bool,
    raw_value: String,
    had_stdin: bool,
}

thread_local! {
    static UNIVERSAL_OBSERVATION: RefCell<Option<UniversalObservation>> = const { RefCell::new(None) };
}

#[universal_tool_middleware(name = "phase-six-universal")]
async fn universal_acceptance(
    tool_name: String,
    tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    principal: Principal,
    mut underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    UNIVERSAL_OBSERVATION.with(|observation| {
        *observation.borrow_mut() = Some(UniversalObservation {
            tool_name,
            tool_version: tool_metadata.version,
            command_path: command_path.clone(),
            anonymous_principal: matches!(principal, Principal::Anonymous),
            raw_value: format!("{:?}", input.value()),
            had_stdin: stdin.is_some(),
        });
    });
    underlying.invoke(command_path, input, stdin).await
}

#[test]
fn universal_middleware_observes_runtime_context_and_forwards_raw_semantics(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let input = canonical_input::<AcceptanceEchoUnderlying>(
            &["echo"],
            vec![SchemaValue::String("semantic".to_string())],
        );
        let expected_graph = input.graph().clone();
        let expected_value = input.value().clone();
        let calls = Rc::new(Cell::new(0));
        let calls_for_fake = Rc::clone(&calls);
        let underlying = UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
            assert_eq!(path, ["echo"]);
            assert!(stdin.is_none());
            let input = decode_typed_schema_value_owned(input).unwrap();
            assert_eq!(input.graph(), &expected_graph);
            assert_eq!(input.value(), &expected_value);
            calls_for_fake.set(calls_for_fake.get() + 1);
            Box::pin(async move { Ok(typed_result("universal-result".to_string())) })
        }));

        let result = invoke(
            "phase-six-universal",
            "runtime-echo",
            {
                let mut tool = AcceptanceEchoUnderlying::__golem_tool_descriptor();
                tool.version = "9.8.7".to_string();
                tool
            },
            vec!["echo".to_string()],
            input,
            None,
            Principal::Anonymous,
            underlying,
        )
        .await
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(
            String::from_value(result.result.unwrap().value()).unwrap(),
            "universal-result"
        );
        assert_eq!(
            UNIVERSAL_OBSERVATION.with(|observation| observation.borrow().clone()),
            Some(UniversalObservation {
                tool_name: "runtime-echo".to_string(),
                tool_version: "9.8.7".to_string(),
                command_path: vec!["echo".to_string()],
                anonymous_principal: true,
                raw_value: "Record { fields: [String(\"semantic\")] }".to_string(),
                had_stdin: false,
            })
        );
    });
}

#[tool_definition]
trait StreamTool {
    fn copy(
        &self,
        input: crate::agentic::InputStream,
        output: crate::agentic::OutputStream,
    ) -> String;
}

struct StreamPolicy;

impl StreamPolicy {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "phase-six-stream-policy",
    constructor = StreamPolicy::new
)]
impl StreamToolMiddleware for StreamPolicy {
    async fn copy(
        &self,
        underlying: &mut StreamToolUnderlying,
        input: InputStream,
    ) -> Result<(String, InputStream), ToolInvokeError<Infallible>> {
        underlying.copy(input).await
    }
}

fn stream_underlying(include_stdout: bool) -> UnderlyingTool {
    UnderlyingTool::from_fake(Box::new(move |path, _input, stdin| {
        assert_eq!(path, ["copy"]);
        Box::pin(async move {
            let bytes = stdin.expect("copy receives stdin").collect().await;
            assert_eq!(bytes, b"request");
            Ok(wire::InvocationResult {
                result: Some(
                    encode_typed_schema_value_owned(
                        "copied".to_string().into_typed_schema_value().unwrap(),
                    )
                    .unwrap(),
                ),
                stdout: include_stdout.then(|| readable_stream(b"response")),
            })
        })
    }))
}

async fn invoke_stream(
    stdin: Option<InputStream>,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<TypedSchemaValue>> {
    invoke(
        "phase-six-stream-policy",
        "stream-tool",
        StreamToolUnderlying::__golem_tool_descriptor(),
        vec!["copy".to_string()],
        canonical_input::<StreamToolUnderlying>(&["copy"], vec![]),
        stdin,
        Principal::Anonymous,
        underlying,
    )
    .await
}

#[test]
fn middleware_transfers_stdin_and_forwards_readable_stdout(
    _registry_test_state: &RegistryTestState,
) {
    TEST_STREAM_NEW_CALLS.store(0, Ordering::Relaxed);
    run_acceptance(async {
        let result = invoke_stream(Some(readable_stream(b"request")), stream_underlying(true))
            .await
            .unwrap();
        assert_eq!(
            String::from_value(result.result.unwrap().value()).unwrap(),
            "copied"
        );
        assert_eq!(result.stdout.unwrap().collect().await, b"response");
    });
    assert_eq!(TEST_STREAM_NEW_CALLS.load(Ordering::Relaxed), 0);
}

#[test]
fn dispatch_rejects_invalid_commands_inputs_results_and_stream_shapes(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let invalid_command = invoke(
            "phase-six-transparent-policy",
            "acceptance-echo",
            AcceptanceEchoUnderlying::__golem_tool_descriptor(),
            vec!["missing".to_string()],
            canonical_input::<AcceptanceEchoUnderlying>(
                &["echo"],
                vec![SchemaValue::String("value".to_string())],
            ),
            None,
            Principal::Anonymous,
            echo_underlying(Rc::new(Cell::new(0)), false),
        )
        .await;
        assert!(matches!(
            invalid_command,
            Err(ToolInvokeError::InvalidCommandPath(path)) if path == ["missing"]
        ));

        let invalid_input = invoke(
            "phase-six-transparent-policy",
            "acceptance-echo",
            AcceptanceEchoUnderlying::__golem_tool_descriptor(),
            vec!["echo".to_string()],
            "not-a-record"
                .to_string()
                .into_typed_schema_value()
                .unwrap(),
            None,
            Principal::Anonymous,
            echo_underlying(Rc::new(Cell::new(0)), false),
        )
        .await;
        assert!(matches!(
            invalid_input,
            Err(ToolInvokeError::InvalidInput(_))
        ));

        let invalid_result = invoke_adapter(
            1,
            UnderlyingTool::from_fake(Box::new(|_, _, _| {
                Box::pin(async { Ok(typed_result("not-a-u64".to_string())) })
            })),
        )
        .await;
        assert!(matches!(
            invalid_result,
            Err(ToolInvokeError::InvalidResult(_))
        ));

        let missing_stdin = invoke_stream(None, stream_underlying(true)).await;
        assert!(matches!(
            missing_stdin,
            Err(ToolInvokeError::InvalidInput(_))
        ));

        let missing_stdout =
            invoke_stream(Some(readable_stream(b"request")), stream_underlying(false)).await;
        assert!(matches!(
            missing_stdout,
            Err(ToolInvokeError::InvalidResult(_))
        ));

        let unexpected_stdin = invoke_echo(
            "forward",
            Some(readable_stream(b"unexpected")),
            echo_underlying(Rc::new(Cell::new(0)), false),
        )
        .await;
        assert!(matches!(
            unexpected_stdin,
            Err(ToolInvokeError::InvalidInput(_))
        ));

        let unexpected_stdout = invoke_echo(
            "forward",
            None,
            UnderlyingTool::from_fake(Box::new(|_, _, _| {
                Box::pin(async {
                    let mut result = typed_result("value".to_string());
                    result.stdout = Some(readable_stream(b"unexpected"));
                    Ok(result)
                })
            })),
        )
        .await;
        assert!(matches!(
            unexpected_stdout,
            Err(ToolInvokeError::InvalidResult(_))
        ));
    });
}

#[tool_definition]
trait CapabilityTool {
    fn carry(&self, capabilities: Vec<(GuestSecretHandle, GuestQuotaTokenHandle)>);
}

struct CapabilityPolicy;

impl CapabilityPolicy {
    fn new() -> Self {
        Self
    }
}

#[tool_middleware(
    name = "phase-six-capability-policy",
    constructor = CapabilityPolicy::new
)]
impl CapabilityToolMiddleware for CapabilityPolicy {
    async fn carry(
        &self,
        underlying: &mut CapabilityToolUnderlying,
        capabilities: Vec<(GuestSecretHandle, GuestQuotaTokenHandle)>,
    ) -> Result<(), ToolInvokeError<Infallible>> {
        underlying.carry(capabilities).await
    }
}

fn capability_input(secret: &GuestSecretHandle, quota: &GuestQuotaTokenHandle) -> TypedSchemaValue {
    canonical_input::<CapabilityToolUnderlying>(
        &["carry"],
        vec![vec![(secret.clone(), quota.clone())].to_value()],
    )
}

fn capability_underlying(seen: Rc<RefCell<(Vec<u32>, Vec<u32>)>>) -> UnderlyingTool {
    UnderlyingTool::from_fake(Box::new(move |path, input, stdin| {
        assert_eq!(path, ["carry"]);
        assert!(stdin.is_none());
        let mut secrets = Vec::new();
        let mut quotas = Vec::new();
        for node in &input.value.value_nodes {
            match node {
                schema_wire::SchemaValueNode::SecretValue(handle) => {
                    secrets.push(handle.take_handle());
                }
                schema_wire::SchemaValueNode::QuotaTokenHandle(handle) => {
                    quotas.push(handle.take_handle());
                }
                _ => {}
            }
        }
        *seen.borrow_mut() = (secrets, quotas);
        Box::pin(async {
            Ok(wire::InvocationResult {
                result: None,
                stdout: None,
            })
        })
    }))
}

fn fresh_capabilities(secret: u32, quota: u32) -> (GuestSecretHandle, GuestQuotaTokenHandle) {
    (
        GuestSecretHandle::new(unsafe { schema_wire::Secret::from_handle(secret) }),
        GuestQuotaTokenHandle::new(unsafe { schema_wire::QuotaToken::from_handle(quota) }),
    )
}

#[test]
fn nested_secret_and_quota_capabilities_cross_monomorphic_and_universal_once(
    _registry_test_state: &RegistryTestState,
) {
    run_acceptance(async {
        let (secret, quota) = fresh_capabilities(51, 52);
        let seen = Rc::new(RefCell::new((Vec::new(), Vec::new())));
        let result = invoke(
            "phase-six-capability-policy",
            "capability-tool",
            CapabilityToolUnderlying::__golem_tool_descriptor(),
            vec!["carry".to_string()],
            capability_input(&secret, &quota),
            None,
            Principal::Anonymous,
            capability_underlying(Rc::clone(&seen)),
        )
        .await
        .unwrap();
        assert!(result.result.is_none());
        assert_eq!(*seen.borrow(), (vec![51], vec![52]));
        assert!(!secret.is_present());
        assert!(!quota.is_present());

        let (secret, quota) = fresh_capabilities(61, 62);
        let seen = Rc::new(RefCell::new((Vec::new(), Vec::new())));
        let result = invoke(
            "phase-six-universal",
            "capability-tool",
            CapabilityToolUnderlying::__golem_tool_descriptor(),
            vec!["carry".to_string()],
            capability_input(&secret, &quota),
            None,
            Principal::Anonymous,
            capability_underlying(Rc::clone(&seen)),
        )
        .await
        .unwrap();
        assert!(result.result.is_none());
        assert_eq!(*seen.borrow(), (vec![61], vec![62]));
        assert!(!secret.is_present());
        assert!(!quota.is_present());
    });
}
