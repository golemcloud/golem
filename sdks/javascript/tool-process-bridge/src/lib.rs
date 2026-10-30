// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0.

use futures::{
    FutureExt,
    future::{Either, select},
};
use golem_rust::{
    bindings::golem::tool::{host, streams},
    golem_schema::tool::{
        process::{ParsedProcessCommand, parse_command, render_custom_error, render_result},
        wit::decode_tool,
    },
    schema::wit::wire::ToolRpcError,
};
use rquickjs::{
    Class, Ctx, Exception, IntoJs, JsLifetime, Object, TypedArray, Value,
    class::Trace,
    convert::List,
    module::{Declarations, Exports, ModuleDef},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    rc::Rc,
};

type Reader = wit_bindgen::StreamReader<host::ByteStreamItem>;

fn error(ctx: &Ctx<'_>, message: impl std::fmt::Display) -> rquickjs::Error {
    Exception::throw_message(ctx, &message.to_string())
}

struct Command {
    name: String,
    path: Vec<String>,
    input: golem_rust::TypedSchemaValue,
    stdin: bool,
    stdout: bool,
    stderr: bool,
    status: bool,
    errors: BTreeMap<String, u8>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class]
pub struct Prepared {
    #[qjs(skip_trace)]
    inner: Rc<Command>,
}

struct InputState {
    writer: RefCell<Option<Rc<host::ToolStdinWriter>>>,
    source: RefCell<Option<host::ToolStdin>>,
    closed: RefCell<Option<host::ToolStdinClosed>>,
    bytes: RefCell<Option<Vec<u8>>>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class]
pub struct Input {
    #[qjs(skip_trace)]
    inner: Rc<InputState>,
}

struct OutputState {
    target: RefCell<Option<host::ToolOutput>>,
    reader: RefCell<Option<Reader>>,
    max: Option<usize>,
    ended: Cell<bool>,
    reading: Cell<bool>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class]
pub struct Output {
    #[qjs(skip_trace)]
    inner: Rc<OutputState>,
}

struct Call {
    _rpc: host::ToolRpc,
    future: host::FutureInvokeResult,
    command: Rc<Command>,
    input: Option<Rc<InputState>>,
    stdout: Option<Rc<OutputState>>,
    stderr: Option<Rc<OutputState>>,
    completed: Cell<bool>,
    collecting: Cell<bool>,
}

impl Drop for Call {
    fn drop(&mut self) {
        if !self.completed.get() {
            self.future.cancel();
        }
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class]
pub struct Process {
    #[qjs(skip_trace)]
    inner: Option<Rc<Call>>,
}

fn output<'js>(
    ctx: &Ctx<'js>,
    code: u8,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> rquickjs::Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("exitCode", code)?;
    object.set("stdout", TypedArray::new(ctx.clone(), stdout)?)?;
    object.set("stderr", TypedArray::new(ctx.clone(), stderr)?)?;
    Ok(object)
}

#[rquickjs::function]
fn prepare_command<'js>(
    ctx: Ctx<'js>,
    name: String,
    args: Vec<String>,
    variables: Vec<List<(String, String)>>,
) -> rquickjs::Result<Value<'js>> {
    let Some(registered) = host::get_tool(&name) else {
        return Ok(Value::new_undefined(ctx));
    };
    let tool =
        decode_tool(registered.definition).map_err(|_| error(&ctx, "invalid tool metadata"))?;
    let parsed = parse_command(
        &tool,
        &args,
        &variables
            .into_iter()
            .map(|entry| entry.0)
            .collect::<HashMap<_, _>>(),
    );
    let object = Object::new(ctx.clone())?;
    match parsed {
        Ok(ParsedProcessCommand::Invoke {
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            result_is_status,
            errors,
        }) => {
            object.set("tag", "invoke")?;
            let spec = Object::new(ctx.clone())?;
            spec.set("stdin", stdin)?;
            spec.set("stdout", stdout)?;
            spec.set("stderr", stderr)?;
            spec.set("commandPath", command_path.clone())?;
            spec.set(
                "input",
                Prepared {
                    inner: Rc::new(Command {
                        name,
                        path: command_path,
                        input: *input,
                        stdin,
                        stdout,
                        stderr,
                        status: result_is_status,
                        errors,
                    }),
                },
            )?;
            object.set("val", spec)?;
        }
        result => {
            object.set("tag", "output")?;
            let rendered = match result {
                Ok(ParsedProcessCommand::Help(text)) => output(&ctx, 0, text.into_bytes(), vec![])?,
                Err(message) => output(&ctx, 2, vec![], format!("{message}\n").into_bytes())?,
                _ => unreachable!(),
            };
            object.set("val", rendered)?;
        }
    }
    object.into_js(&ctx)
}

#[rquickjs::function]
fn create_input(ctx: Ctx<'_>, bytes: Option<TypedArray<'_, u8>>) -> rquickjs::Result<Input> {
    let bytes = bytes
        .map(|bytes| {
            // SAFETY: the bytes are copied before any JavaScript runs.
            unsafe { bytes.as_bytes() }
                .map(Vec::from)
                .ok_or_else(|| error(&ctx, "detached stdin buffer"))
        })
        .transpose()?;
    let (writer, source, closed) = host::create_stdin();
    let closed = if bytes.is_some() {
        drop(closed);
        None
    } else {
        Some(closed)
    };
    Ok(Input {
        inner: Rc::new(InputState {
            writer: RefCell::new(Some(Rc::new(writer))),
            source: RefCell::new(Some(source)),
            closed: RefCell::new(closed),
            bytes: RefCell::new(bytes),
        }),
    })
}

#[rquickjs::function]
fn create_output(max: Option<usize>) -> Output {
    let (target, reader) = host::create_output();
    Output {
        inner: Rc::new(OutputState {
            target: RefCell::new(Some(target)),
            reader: RefCell::new(Some(reader)),
            max,
            ended: Cell::new(false),
            reading: Cell::new(false),
        }),
    }
}

#[rquickjs::function]
fn invoke(
    ctx: Ctx<'_>,
    prepared: Prepared,
    input: Option<Input>,
    stdout: Option<Output>,
    stderr: Option<Output>,
) -> rquickjs::Result<Process> {
    let command = prepared.inner;
    if command.stdin != input.is_some()
        || command.stdout != stdout.is_some()
        || command.stderr != stderr.is_some()
    {
        return Err(error(&ctx, "invalid tool process attachments"));
    }
    let value = golem_rust::encode_typed_schema_value(&command.input)
        .map_err(|_| error(&ctx, "invalid canonical input"))?;
    let input = input.map(|input| input.inner);
    let stdout = stdout.map(|output| output.inner);
    let stderr = stderr.map(|output| output.inner);
    let rpc = host::ToolRpc::new(&command.name);
    let future = rpc.async_invoke_and_await(
        &command.path,
        value,
        input
            .as_ref()
            .and_then(|input| input.source.borrow_mut().take()),
        stdout
            .as_ref()
            .and_then(|output| output.target.borrow_mut().take()),
        stderr
            .as_ref()
            .and_then(|output| output.target.borrow_mut().take()),
    );
    Ok(Process {
        inner: Some(Rc::new(Call {
            _rpc: rpc,
            future,
            command,
            input,
            stdout,
            stderr,
            completed: Cell::new(false),
            collecting: Cell::new(false),
        })),
    })
}

#[rquickjs::function]
fn cancel(process: Process) {
    if let Some(call) = process.inner
        && !call.completed.get()
    {
        call.future.cancel();
    }
}

#[rquickjs::function]
fn dispose_process(process: Class<'_, Process>) {
    process.borrow_mut().inner.take();
}

fn write_error(ctx: &Ctx<'_>, reason: streams::StreamWriteError) -> rquickjs::Error {
    let value = match Object::new(ctx.clone()).and_then(|value| {
        value.set(
            "tag",
            match reason {
                streams::StreamWriteError::Closed(_) => "closed",
                streams::StreamWriteError::ConcurrentOperation => "concurrent-operation",
            },
        )?;
        Ok(value)
    }) {
        Ok(value) => value,
        Err(error) => return error,
    };
    ctx.throw(value.into_value())
}

#[rquickjs::function]
async fn write<'js>(
    ctx: Ctx<'js>,
    input: Input,
    bytes: TypedArray<'js, u8>,
) -> rquickjs::Result<()> {
    // SAFETY: the bytes are copied before any JavaScript runs.
    let bytes = unsafe { bytes.as_bytes() }
        .ok_or_else(|| error(&ctx, "detached stdin buffer"))?
        .to_vec();
    let writer = input
        .inner
        .writer
        .borrow()
        .clone()
        .ok_or_else(|| error(&ctx, "stdin closed"))?;
    for chunk in bytes.chunks(32 * 1024) {
        writer
            .write(chunk.to_vec())
            .await
            .map_err(|reason| write_error(&ctx, reason))?;
    }
    Ok(())
}

#[rquickjs::function]
async fn finish(ctx: Ctx<'_>, input: Input, failed: bool) -> rquickjs::Result<()> {
    let writer = input.inner.writer.borrow_mut().take();
    if let Some(writer) = writer {
        let result = if failed {
            writer.fail(streams::ByteStreamFailure::Abandoned).await
        } else {
            writer.finish().await
        };
        result.map_err(|reason| write_error(&ctx, reason))?;
    }
    Ok(())
}

#[rquickjs::function]
async fn wait_input_closed(input: Input) {
    let closed = input.inner.closed.borrow_mut().take();
    if let Some(closed) = closed {
        closed.wait().await;
    }
}

#[rquickjs::function]
fn dispose_input(input: Input, part: String) {
    match part.as_str() {
        "writer" => {
            input.inner.writer.borrow_mut().take();
        }
        "source" => {
            input.inner.source.borrow_mut().take();
        }
        "closed" => {
            input.inner.closed.borrow_mut().take();
        }
        _ => {}
    }
}

#[rquickjs::function]
fn dispose_output(output: Output, target: bool) {
    if target {
        output.inner.target.borrow_mut().take();
    } else {
        output.inner.ended.set(true);
        output.inner.reader.borrow_mut().take();
    }
}

struct ReadGuard {
    output: Rc<OutputState>,
    reader: Option<Reader>,
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        self.output.reading.set(false);
        if !self.output.ended.get() {
            *self.output.reader.borrow_mut() = self.reader.take();
        }
    }
}

#[rquickjs::function]
async fn read_output<'js>(
    ctx: Ctx<'js>,
    output: Output,
) -> rquickjs::Result<Option<TypedArray<'js, u8>>> {
    if output.inner.ended.get() {
        return Ok(None);
    }
    if output.inner.reading.replace(true) {
        return Err(error(&ctx, "concurrent output read"));
    }
    let reader = output.inner.reader.borrow_mut().take();
    let mut guard = ReadGuard {
        output: output.inner,
        reader,
    };
    let Some(reader) = guard.reader.as_mut() else {
        return Err(error(&ctx, "output reader unavailable"));
    };
    match reader.next().await {
        Some(Ok(bytes)) => Ok(Some(TypedArray::new(ctx, bytes)?)),
        Some(Err(reason)) => {
            guard.output.ended.set(true);
            let value = Object::new(ctx.clone())?;
            value.set(
                "tag",
                if matches!(reason, streams::ByteStreamFailure::Cancelled) {
                    "cancelled"
                } else {
                    "failed"
                },
            )?;
            Err(ctx.throw(value.into_value()))
        }
        None => {
            guard.output.ended.set(true);
            Ok(None)
        }
    }
}

#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    overflow: bool,
    failure: Option<&'static str>,
}

async fn drain(output: Option<Rc<OutputState>>, future: &host::FutureInvokeResult) -> Collected {
    let mut result = Collected::default();
    let Some(output) = output else {
        return result;
    };
    let reader = output.reader.borrow_mut().take();
    let Some(mut reader) = reader else {
        return result;
    };
    while let Some(item) = reader.next().await {
        match item {
            Ok(chunk) => {
                if let Some(max) = output.max {
                    let remaining = max - result.bytes.len();
                    result
                        .bytes
                        .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                    if chunk.len() > remaining {
                        result.overflow = true;
                        future.cancel();
                        break;
                    }
                }
            }
            Err(reason) => {
                result.failure = Some(if matches!(reason, streams::ByteStreamFailure::Cancelled) {
                    "cancelled"
                } else {
                    "failed"
                });
                break;
            }
        }
    }
    result
}

fn rpc_error(error: ToolRpcError, declared: &BTreeMap<String, u8>) -> (u8, String, &'static str) {
    use golem_rust::schema::wit::wire::ToolError;
    match error {
        ToolRpcError::Cancelled => (130, "Cancelled".into(), "cancelled"),
        ToolRpcError::Denied(message) => (3, message, "denied"),
        ToolRpcError::NotFound(message) => (127, message, "not-found"),
        ToolRpcError::RemoteToolError(
            ToolError::InvalidInput(message) | ToolError::ConstraintViolation(message),
        ) => (2, message, "remote-tool-error"),
        ToolRpcError::RemoteToolError(ToolError::CustomError(custom)) => {
            let payload = golem_rust::decode_typed_schema_value(&custom.payload).ok();
            let (code, message) = render_custom_error(&custom.name, payload.as_ref(), declared);
            (code, message, "remote-tool-error")
        }
        ToolRpcError::ProtocolError(message)
        | ToolRpcError::RemoteInternalError(message)
        | ToolRpcError::ResourceExhausted(message) => (1, message, "rpc-error"),
        _ => (1, "tool call failed".into(), "rpc-error"),
    }
}

struct Collection {
    terminal: Result<host::InvocationResult, ToolRpcError>,
    pumped: Result<(), ()>,
    stdout: Collected,
    stderr: Collected,
    timed_out: bool,
}

async fn collect_inner(
    call: Rc<Call>,
    timeout: Option<u64>,
    collect_outputs: bool,
) -> Result<Collection, &'static str> {
    if call.collecting.replace(true) {
        return Err("tool process collection already started");
    }
    let pump = async {
        if let Some(input) = &call.input {
            let bytes = input.bytes.borrow_mut().take();
            if let Some(bytes) = bytes {
                let writer = input.writer.borrow_mut().take();
                if let Some(writer) = writer {
                    for chunk in bytes.chunks(32 * 1024) {
                        writer.write(chunk.to_vec()).await.map_err(|_| ())?;
                    }
                    writer.finish().await.map_err(|_| ())?;
                }
            }
        }
        Ok::<_, ()>(())
    };
    let mut timed_out = false;
    let outputs = async {
        if collect_outputs {
            futures::join!(
                drain(call.stdout.clone(), &call.future),
                drain(call.stderr.clone(), &call.future)
            )
        } else {
            (Collected::default(), Collected::default())
        }
    };
    let collected = async { futures::join!(call.future.get(), pump, outputs) };
    let (terminal, pumped, (stdout, stderr)) = if let Some(timeout) = timeout {
        let wait = golem_rust::wasip3::clocks::monotonic_clock::wait_for(
            timeout.saturating_mul(1_000_000),
        );
        match select(collected.boxed_local(), wait.boxed_local()).await {
            Either::Left((result, _)) => result,
            Either::Right((_, collected)) => {
                timed_out = true;
                call.future.cancel();
                collected.await
            }
        }
    } else {
        collected.await
    };
    call.completed.set(true);
    Ok(Collection {
        terminal,
        pumped,
        stdout,
        stderr,
        timed_out,
    })
}

fn render_collection<'js>(
    ctx: Ctx<'js>,
    call: &Call,
    collection: Collection,
) -> rquickjs::Result<Object<'js>> {
    let Collection {
        terminal,
        pumped,
        stdout,
        stderr,
        timed_out,
    } = collection;
    let result = Object::new(ctx.clone())?;
    let (rendered, tag, failure) = match terminal {
        Ok(value) => {
            if value.stdout.is_some() || value.stderr.is_some() {
                return Err(error(&ctx, "unexpected terminal output attachment"));
            }
            let decoded = value
                .result
                .as_ref()
                .map(golem_rust::decode_typed_schema_value)
                .transpose();
            let rendered = decoded
                .map_err(|_| "invalid tool result".to_string())
                .and_then(|value| {
                    render_result(call.command.stdout, call.command.status, value.as_ref())
                });
            match rendered {
                Ok(rendered) => (
                    output(
                        &ctx,
                        rendered.exit_code,
                        rendered.stdout.unwrap_or_default(),
                        vec![],
                    )?,
                    "ok",
                    None,
                ),
                Err(message) => (
                    output(&ctx, 1, vec![], format!("{message}\n").into_bytes())?,
                    "ok",
                    None,
                ),
            }
        }
        Err(reason) => {
            let (code, message, reason) = rpc_error(reason, &call.command.errors);
            (
                output(&ctx, code, vec![], format!("{message}\n").into_bytes())?,
                "err",
                Some(reason),
            )
        }
    };
    result.set("tag", tag)?;
    if let Some(failure) = failure {
        let value = Object::new(ctx.clone())?;
        value.set("tag", failure)?;
        result.set("val", value)?;
    }
    result.set("rendered", rendered)?;
    let collected = Object::new(ctx.clone())?;
    collected.set("result", result)?;
    collected.set("timedOut", timed_out)?;
    for (name, mut stream) in [("stdout", stdout), ("stderr", stderr)] {
        if pumped.is_err() && failure.is_none() {
            stream.failure.get_or_insert("failed");
        }
        let object = Object::new(ctx.clone())?;
        object.set("bytes", TypedArray::new(ctx.clone(), stream.bytes)?)?;
        object.set("overflow", stream.overflow)?;
        if let Some(failure) = stream.failure {
            let value = Object::new(ctx.clone())?;
            value.set("tag", failure)?;
            object.set("failure", value)?;
        }
        collected.set(name, object)?;
    }
    Ok(collected)
}

#[rquickjs::function]
fn collect_sync<'js>(
    ctx: Ctx<'js>,
    process: Process,
    timeout: Option<u64>,
) -> rquickjs::Result<Object<'js>> {
    let call = process
        .inner
        .ok_or_else(|| error(&ctx, "tool process disposed"))?;
    let collected = golem_rust::wasip3::wit_bindgen::rt::async_support::block_on(collect_inner(
        call.clone(),
        timeout,
        true,
    ))
    .map_err(|message| error(&ctx, message))?;
    render_collection(ctx, &call, collected)
}

#[rquickjs::function]
async fn collect_async<'js>(ctx: Ctx<'js>, process: Process) -> rquickjs::Result<Object<'js>> {
    let call = process
        .inner
        .ok_or_else(|| error(&ctx, "tool process disposed"))?;
    let collected = collect_inner(call.clone(), None, false)
        .await
        .map_err(|message| error(&ctx, message))?;
    render_collection(ctx, &call, collected)
}

pub struct ToolProcessBridge;
impl ModuleDef for ToolProcessBridge {
    fn declare(decl: &Declarations) -> rquickjs::Result<()> {
        for name in [
            "prepareCommand",
            "createInput",
            "createOutput",
            "invoke",
            "cancel",
            "write",
            "finish",
            "waitInputClosed",
            "disposeInput",
            "disposeOutput",
            "disposeProcess",
            "collectSync",
            "collectAsync",
            "readOutput",
        ] {
            decl.declare(name)?;
        }
        Ok(())
    }
    fn evaluate<'js>(_: &Ctx<'js>, exports: &Exports<'js>) -> rquickjs::Result<()> {
        exports.export("prepareCommand", js_prepare_command)?;
        exports.export("createInput", js_create_input)?;
        exports.export("createOutput", js_create_output)?;
        exports.export("invoke", js_invoke)?;
        exports.export("cancel", js_cancel)?;
        exports.export("write", js_write)?;
        exports.export("finish", js_finish)?;
        exports.export("waitInputClosed", js_wait_input_closed)?;
        exports.export("disposeInput", js_dispose_input)?;
        exports.export("disposeOutput", js_dispose_output)?;
        exports.export("disposeProcess", js_dispose_process)?;
        exports.export("collectSync", js_collect_sync)?;
        exports.export("collectAsync", js_collect_async)?;
        exports.export("readOutput", js_read_output)?;
        Ok(())
    }
}
