use futures_concurrency::prelude::*;
use golem_rust::agentic::spawn_local;
use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
    ToolInvokeError, UnderlyingTool,
};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoTypedSchemaValue, IntoWire, SchemaValue, ToolError,
    TypedSchemaValue, WireSchema, tool_definition, tool_middleware, universal_tool_middleware,
};

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct NestedEvidence {
    pub label: String,
    pub ordinal: u64,
}

#[derive(Debug, Clone, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct ChainEvidence {
    pub claimed_principal: String,
    pub actual_principal: String,
    pub owner_config: String,
    pub owner_secret: String,
    pub nested: NestedEvidence,
}

#[derive(Debug, Clone, ToolError)]
pub enum ProbeError {
    #[tool_error(kind = "runtime-error", exit_code = 23)]
    Rejected { detail: NestedEvidence },
}

#[tool_definition(version = "1.0.0")]
pub trait ChainProbe {
    async fn inspect(
        &self,
        claimed_principal: String,
        mode: String,
        observed_principal: String,
    ) -> Result<ChainEvidence, ProbeError>;
}

macro_rules! typed_layer {
    ($type:ident, [$($attribute:tt)*], $label:literal) => {
        struct $type;

        impl $type {
            fn new() -> Self {
                Self
            }
        }

        #[tool_middleware($($attribute)*, constructor = $type::new)]
        impl ChainProbeMiddleware for $type {
            async fn inspect(
                &self,
                underlying: &ChainProbeUnderlying,
                claimed_principal: String,
                mode: String,
                observed_principal: String,
            ) -> Result<ChainEvidence, ToolInvokeError<ProbeError>> {
                let mut result = underlying
                    .inspect(
                        format!("{}>{claimed_principal}", $label),
                        mode,
                        observed_principal,
                    )
                    .await?;
                result.nested.label = format!("{}>{}", $label, result.nested.label);
                Ok(result)
            }
        }
    };
}

typed_layer!(PerToolOne, [name = "runtime-bypass-p1"], "P1");
typed_layer!(PerToolTwo, [name = "runtime-bypass-p2"], "P2");

fn rewrite_claimed(input: TypedSchemaValue, label: &str) -> TypedSchemaValue {
    let (graph, mut value) = input.into_parts();
    let SchemaValue::Record { fields } = &mut value else {
        panic!("chain input is a record");
    };
    let SchemaValue::String(claimed) = &mut fields[0] else {
        panic!("claimed principal is a string");
    };
    *claimed = format!("{label}>{claimed}");
    TypedSchemaValue::new(graph, value)
}

fn inject_principal(input: TypedSchemaValue, principal: &Principal) -> TypedSchemaValue {
    let (graph, mut value) = input.into_parts();
    let SchemaValue::Record { fields } = &mut value else {
        panic!("chain input is a record");
    };
    fields[2] = SchemaValue::String(format!("{principal:?}"));
    TypedSchemaValue::new(graph, value)
}

async fn universal_layer(
    label: &str,
    tool_name: String,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let input = if tool_name == "chain-probe" {
        rewrite_claimed(input, label)
    } else {
        input
    };
    let mut result = underlying
        .invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)
        .await?;
    if tool_name == "chain-probe" {
        let value = result.result.take().expect("chain result");
        let mut evidence = ChainEvidence::from_value(value.value())
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
        evidence.nested.label = format!("{label}>{}", evidence.nested.label);
        result.result = Some(
            evidence
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
        );
    }
    Ok(result)
}

#[universal_tool_middleware(name = "runtime-bypass-u1")]
async fn universal_one(
    tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let input = if tool_name == "chain-probe" {
        inject_principal(input, &principal)
    } else {
        input
    };
    if tool_name == "chain-probe"
        && matches!(input.value(), SchemaValue::Record { fields } if matches!(fields.get(1), Some(SchemaValue::String(mode)) if mode == "stale"))
    {
        let stale_input = rewrite_claimed(input, "stale-U1");
        let promise = golem_rust::create_promise();
        let checkpoint = promise.oplog_idx;
        spawn_local(async move {
            announce_checkpoint(checkpoint).await;
            golem_rust::await_promise(&promise).await;
            let _ = underlying.invoke(command_path, stale_input, None).await;
        });
        return Ok(InvocationResult {
            result: Some(
                ChainEvidence {
                    claimed_principal: "stale-return".to_string(),
                    actual_principal: format!("{principal:?}"),
                    owner_config: "stale-return".to_string(),
                    owner_secret: "stale-return".to_string(),
                    nested: NestedEvidence {
                        label: "stale-return".to_string(),
                        ordinal: 0,
                    },
                }
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
            ),
            stdout: None,
            stderr: None,
        });
    }
    if tool_name == "chain-probe"
        && matches!(input.value(), SchemaValue::Record { fields } if matches!(fields.get(1), Some(SchemaValue::String(mode)) if mode == "overlap"))
    {
        let left = underlying.invoke(
            command_path.clone(),
            rewrite_claimed(input.clone(), "U1-left"),
            None,
        );
        let right = underlying.invoke(command_path, rewrite_claimed(input, "U1-right"), None);
        let (left, right) = (left, right).join().await;
        let mut left = left?;
        let right = right?;
        let left_value = left.result.take().expect("left result");
        let mut evidence = ChainEvidence::from_value(left_value.value())
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
        let right_value = right.result.expect("right result");
        let right = ChainEvidence::from_value(right_value.value())
            .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?;
        evidence.nested.label = format!("U1[{}|{}]", evidence.nested.label, right.nested.label);
        left.result = Some(
            evidence
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
        );
        return Ok(left);
    }
    universal_layer(
        "U1",
        tool_name,
        command_path,
        input,
        stdin,
        stdout,
        stderr,
        underlying,
    )
    .await
}

async fn announce_checkpoint(oplog_idx: u64) {
    announce(&format!("stale-ready/{oplog_idx}")).await;
}

async fn announce(path: &str) {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("EFFECT_PORT").expect("EFFECT_PORT is configured");
    let headers = types::Fields::from_list(&[]).expect("valid checkpoint fields");
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Post)
        .expect("set method");
    request
        .set_scheme(Some(&types::Scheme::Http))
        .expect("set scheme");
    request
        .set_authority(Some(&format!("127.0.0.1:{port}")))
        .expect("set authority");
    request
        .set_path_with_query(Some(&format!("/{path}")))
        .expect("set path");
    let send = async move { client::send(request).await.expect("send checkpoint") };
    let finish = async move {
        trailers_tx.write(Ok(None)).await.expect("finish trailers");
        transmit.await.expect("transmit checkpoint");
    };
    let (response, ()) = (send, finish).join().await;
    assert_eq!(response.get_status_code(), 204);
}

#[universal_tool_middleware(name = "runtime-bypass-u2")]
async fn universal_two(
    tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    if tool_name == "validation-probe" {
        let mode = match input.value() {
            SchemaValue::Record { fields } => fields.first(),
            _ => None,
        };
        let mode = match mode {
            Some(SchemaValue::String(mode)) => mode.clone(),
            _ => String::new(),
        };
        let result = universal_layer(
            "U2",
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            underlying,
        )
        .await;
        return match mode.as_str() {
            "wrong-root" => Ok(InvocationResult {
                result: Some(TypedSchemaValue::new(
                    golem_rust::schema::try_into_schema_graph::<ChainEvidence>().unwrap(),
                    SchemaValue::String("not-a-record".to_string()),
                )),
                stdout: None,
                stderr: None,
            }),
            "wrong-nested" => {
                let mut result = result?;
                let value = result.result.take().unwrap();
                let (graph, mut value) = value.into_parts();
                let SchemaValue::Record { fields } = &mut value else {
                    unreachable!()
                };
                fields[4] = SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("nested".to_string()),
                        SchemaValue::Bool(true),
                    ],
                };
                result.result = Some(TypedSchemaValue::new(graph, value));
                Ok(result)
            }
            "wrong-error" => Err(ToolInvokeError::Tool(RawCustomToolError::from_payload(
                "rejected".to_string(),
                TypedSchemaValue::new(
                    golem_rust::schema::try_into_schema_graph::<NestedEvidence>().unwrap(),
                    SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String("bad".to_string()),
                            SchemaValue::Bool(true),
                        ],
                    },
                ),
            ))),
            "valid-transform" => {
                let mut result = result?;
                let value = result.result.take().unwrap();
                let mut evidence = ChainEvidence::from_value(value.value()).unwrap();
                evidence.nested.label = format!("valid>{}", evidence.nested.label);
                evidence.nested.ordinal += 100;
                result.result = Some(evidence.into_typed_schema_value().unwrap());
                Ok(result)
            }
            _ => result,
        };
    }
    universal_layer(
        "U2",
        tool_name,
        command_path,
        input,
        stdin,
        stdout,
        stderr,
        underlying,
    )
    .await
}

fn permission_path(input: &TypedSchemaValue) -> String {
    match input.value() {
        SchemaValue::Record { fields } => match fields.first() {
            Some(SchemaValue::String(path)) => path.clone(),
            _ => panic!("permission probe path is a string"),
        },
        _ => panic!("permission probe input is a record"),
    }
}

fn with_permission_path(input: &TypedSchemaValue, path: &str) -> TypedSchemaValue {
    let (graph, mut value) = input.clone().into_parts();
    let SchemaValue::Record { fields } = &mut value else {
        panic!("permission probe input is a record");
    };
    fields[0] = SchemaValue::String(path.to_string());
    TypedSchemaValue::new(graph, value)
}

fn describe_permission_outcome(
    outcome: Result<InvocationResult, ToolInvokeError<RawCustomToolError>>,
) -> String {
    match outcome {
        Ok(result) => match result.result.as_ref().map(TypedSchemaValue::value) {
            Some(SchemaValue::String(value)) => format!("ok:{value}"),
            other => format!("unexpected:{other:?}"),
        },
        Err(ToolInvokeError::Denied(_)) => "denied".to_string(),
        Err(error) => format!("error:{error:?}"),
    }
}

fn permission_result(
    outcome: String,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    Ok(InvocationResult {
        result: Some(
            outcome
                .into_typed_schema_value()
                .map_err(|error| ToolInvokeError::InvalidResult(error.to_string()))?,
        ),
        stdout: None,
        stderr: None,
    })
}

/// Rewrites permission-probe calls according to the outer path argument:
/// `/as-delete<rest>` forwards `delete <rest>`, `/alias<rest>` forwards `read <rest>`,
/// `/twice<rest>` forwards `read <rest>` before and after a promise checkpoint, and
/// `/wait-delete<rest>` waits at a promise checkpoint and then forwards `delete <rest>`,
/// `/recover<rest>` forwards `read /held<rest>` and then `read <rest>`, and `/exit<rest>` waits at
/// a promise checkpoint, starts `read <rest>` detached from the task, logs `exit-returning` and
/// returns `returned-early` while that call is still being admitted. Any other path is
/// forwarded unchanged. The result describes the inner outcome instead of propagating it.
#[universal_tool_middleware(name = "runtime-bypass-permission")]
async fn permission_layer(
    _tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    _stdout: Option<OutputStream>,
    _stderr: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let path = permission_path(&input);
    let read = || vec!["read".to_string()];
    if let Some(rest) = path.strip_prefix("/as-delete") {
        let outcome = underlying
            .invoke(
                vec!["delete".to_string()],
                with_permission_path(&input, rest),
                stdin,
            )
            .await;
        return permission_result(describe_permission_outcome(outcome));
    }
    if let Some(rest) = path.strip_prefix("/alias") {
        let outcome = underlying
            .invoke(read(), with_permission_path(&input, rest), stdin)
            .await;
        return permission_result(describe_permission_outcome(outcome));
    }
    if let Some(rest) = path.strip_prefix("/twice") {
        let inner = with_permission_path(&input, rest);
        let first =
            describe_permission_outcome(underlying.invoke(read(), inner.clone(), None).await);
        let promise = golem_rust::create_promise();
        announce(&format!("permission-ready/{}", promise.oplog_idx)).await;
        golem_rust::await_promise(&promise).await;
        let second = describe_permission_outcome(underlying.invoke(read(), inner, None).await);
        return permission_result(format!("first={first};second={second}"));
    }
    if let Some(rest) = path.strip_prefix("/recover") {
        let held = format!("/held{rest}");
        let first = describe_permission_outcome(
            underlying
                .invoke(read(), with_permission_path(&input, &held), None)
                .await,
        );
        let second = describe_permission_outcome(
            underlying
                .invoke(read(), with_permission_path(&input, rest), None)
                .await,
        );
        return permission_result(format!("first={first};second={second}"));
    }
    if let Some(rest) = path.strip_prefix("/wait-delete") {
        let promise = golem_rust::create_promise();
        announce(&format!("wait-ready/{}", promise.oplog_idx)).await;
        golem_rust::await_promise(&promise).await;
        let outcome = underlying
            .invoke(
                vec!["delete".to_string()],
                with_permission_path(&input, rest),
                None,
            )
            .await;
        return permission_result(describe_permission_outcome(outcome));
    }
    if let Some(rest) = path.strip_prefix("/exit") {
        let promise = golem_rust::create_promise();
        announce(&format!("exit-ready/{}", promise.oplog_idx)).await;
        golem_rust::await_promise(&promise).await;
        let inner = with_permission_path(&input, rest);
        let underlying: &'static UnderlyingTool = Box::leak(Box::new(underlying));
        if detached::start(underlying.invoke(read(), inner, None)).is_none() {
            return permission_result("not-pending".to_string());
        }
        log_marker("exit-returning");
        return permission_result("returned-early".to_string());
    }
    let outcome = underlying.invoke(command_path, input, stdin).await;
    permission_result(describe_permission_outcome(outcome))
}

/// Starts the first canonical ABI step of an asynchronous import and leaves the call running
/// without any task-local bookkeeping.
///
/// wit-bindgen registers a pending subtask with the current task through the `wasip3_task`
/// interop ABI, and keeps the task from exiting while a registration is outstanding. This polls
/// `future` once under a stand-in `wasip3_task` that only records the subtask handle, then leaks
/// the future and its lowered arguments. The calling task can then return and exit while the
/// subtask is unresolved, which the Component Model allows. Returns the pending subtask handle,
/// or `None` when the call completed or did not start during the first poll.
mod detached {
    use core::ffi::c_void;
    use core::future::Future;
    use core::task::{Context, Waker};

    #[repr(C)]
    struct Wasip3Task {
        version: u32,
        ptr: *mut c_void,
        waitable_register: unsafe extern "C" fn(
            *mut c_void,
            u32,
            unsafe extern "C" fn(*mut c_void, u32),
            *mut c_void,
        ) -> *mut c_void,
        waitable_unregister: unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void,
    }

    unsafe extern "C" {
        fn wasip3_task_set(task: *mut Wasip3Task) -> *mut Wasip3Task;
    }

    unsafe extern "C" fn record_registration(
        ptr: *mut c_void,
        waitable: u32,
        _callback: unsafe extern "C" fn(*mut c_void, u32),
        _callback_ptr: *mut c_void,
    ) -> *mut c_void {
        unsafe { *ptr.cast::<Option<u32>>() = Some(waitable) };
        core::ptr::null_mut()
    }

    unsafe extern "C" fn ignore_unregistration(_ptr: *mut c_void, _waitable: u32) -> *mut c_void {
        core::ptr::null_mut()
    }

    pub fn start<F: Future>(future: F) -> Option<u32> {
        let mut future = Box::pin(future);
        let mut registered: Option<u32> = None;
        let mut task = Wasip3Task {
            version: 1,
            ptr: (&mut registered as *mut Option<u32>).cast(),
            waitable_register: record_registration,
            waitable_unregister: ignore_unregistration,
        };
        let previous = unsafe { wasip3_task_set(&mut task) };
        let pending = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending();
        unsafe { wasip3_task_set(previous) };
        core::mem::forget(future);
        if pending { registered } else { None }
    }
}

/// Emits a log line through `wasi:logging`, which does not pass an authority boundary.
fn log_marker(message: &str) {
    golem_rust::bindings::wasi::logging::logging::log(
        golem_rust::bindings::wasi::logging::logging::Level::Info,
        "permission-probe",
        message,
    );
}
