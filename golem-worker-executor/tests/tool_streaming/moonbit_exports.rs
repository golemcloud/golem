// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use anyhow::{Context, Result, bail};
use golem_common::model::agent::Principal;
use golem_common::schema::agent::wit::wire as agent_wire;
use golem_common::schema::tool::Tool;
use golem_common::schema::tool::wit::wire as tool_wire;
use golem_common::schema::wit::{decode_value, encode_typed, wire as schema_wire};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache;
use golem_schema::schema::SchemaValueStreamHandleRep;
use golem_schema::schema::wit::{PermissionCardHandleRep, QuotaTokenHandleRep, SecretHandleRep};
use golem_worker_executor_test_utils::{PrecompiledComponent, WorkerExecutorTestDependencies};
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use test_r::{inherit_test_dep, test, timeout};
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{
    Component, ComponentType, Destination, Func, Lift, Linker, Lower, Resource, ResourceType,
    StreamProducer, StreamReader, StreamResult, VecBuffer,
};
use wasmtime::{Engine, Store};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("tool_streaming_moonbit")]
    PrecompiledComponent
);

const INTERFACE: &str = "golem:tool/guest@0.1.0";

#[derive(Debug)]
struct StdoutWriter;

struct UnusedResource;

#[derive(Debug, Clone, PartialEq, ComponentType, Lift, Lower)]
#[component(variant)]
enum ByteStreamFailure {
    #[component(name = "cancelled")]
    Cancelled,
    #[component(name = "abandoned")]
    Abandoned,
    #[component(name = "resource-exhausted")]
    ResourceExhausted,
    #[component(name = "failed")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, ComponentType, Lift, Lower)]
#[component(variant)]
enum ByteStreamCloseCause {
    #[component(name = "finished")]
    Finished,
    #[component(name = "failed")]
    Failed(ByteStreamFailure),
    #[component(name = "consumer-cancelled")]
    ConsumerCancelled,
}

#[derive(ComponentType, Lift, Lower)]
#[component(variant)]
enum StreamWriteError {
    #[component(name = "closed")]
    Closed(ByteStreamCloseCause),
    #[component(name = "concurrent-operation")]
    ConcurrentOperation,
}

#[derive(Default)]
struct Host {
    next_writer: u32,
    writers: HashMap<u32, Arc<Mutex<OutputState>>>,
}

#[derive(Debug, Default)]
struct OutputState {
    bytes: Vec<u8>,
    terminal: Option<ByteStreamCloseCause>,
    drops: usize,
}

struct PendingInput(Arc<AtomicUsize>);

impl Drop for PendingInput {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl StreamProducer<Host> for PendingInput {
    type Item = Result<Vec<u8>, ByteStreamFailure>;
    type Buffer = VecBuffer<Self::Item>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _store: wasmtime::StoreContextMut<'a, Host>,
        _destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish {
            Poll::Ready(Ok(StreamResult::Cancelled))
        } else {
            Poll::Pending
        }
    }
}

type Invocation = Result<tool_wire::InvocationResult, tool_wire::ToolError>;

fn exported(
    store: &mut Store<Host>,
    instance: &wasmtime::component::Instance,
    name: &str,
) -> Result<Func> {
    let (_, interface) = instance
        .get_export(&mut *store, None, INTERFACE)
        .context("tool guest interface")?;
    let (_, function) = instance
        .get_export(&mut *store, Some(&interface), name)
        .with_context(|| format!("tool guest export {name}"))?;
    instance
        .get_func(&mut *store, function)
        .with_context(|| format!("tool guest function {name}"))
}

async fn instantiate(path: &Path) -> Result<(Store<Host>, wasmtime::component::Instance)> {
    let engine = Engine::new(&create_wasmtime_config_without_fs_cache())?;
    let component = Component::from_file(&engine, path)?;
    let mut linker: Linker<Host> = Linker::new(&engine);
    linker.allow_shadowing(true);

    let mut streams = linker.instance("golem:tool/streams@0.1.0")?;
    streams.resource(
        "tool-stdout-writer",
        ResourceType::host::<StdoutWriter>(),
        |mut store, rep| {
            let output = store
                .data_mut()
                .writers
                .remove(&rep)
                .expect("writer dropped twice");
            let mut output = output.lock().unwrap();
            output.drops += 1;
            output
                .terminal
                .get_or_insert(ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned));
            Ok(())
        },
    )?;
    streams.func_wrap_concurrent(
        "[method]tool-stdout-writer.write",
        |accessor, (writer, bytes): (Resource<StdoutWriter>, Vec<u8>)| {
            Box::pin(async move {
                let result = accessor.with(|mut store| {
                    let mut output = store.data_mut().writers[&writer.rep()].lock().unwrap();
                    if let Some(cause) = &output.terminal {
                        return Err(StreamWriteError::Closed(cause.clone()));
                    }
                    output.bytes.extend(bytes);
                    Ok(())
                });
                Ok((result,))
            })
        },
    )?;
    streams.func_wrap_concurrent(
        "[method]tool-stdout-writer.finish",
        |accessor, (writer,): (Resource<StdoutWriter>,)| {
            Box::pin(async move {
                let result = accessor.with(|mut store| {
                    let mut output = store.data_mut().writers[&writer.rep()].lock().unwrap();
                    if let Some(cause) = &output.terminal {
                        return Err(StreamWriteError::Closed(cause.clone()));
                    }
                    output.terminal = Some(ByteStreamCloseCause::Finished);
                    Ok(())
                });
                Ok((result,))
            })
        },
    )?;
    streams.func_wrap_concurrent(
        "[method]tool-stdout-writer.fail",
        |accessor, (writer, failure): (Resource<StdoutWriter>, ByteStreamFailure)| {
            Box::pin(async move {
                let result = accessor.with(|mut store| {
                    let mut output = store.data_mut().writers[&writer.rep()].lock().unwrap();
                    if let Some(cause) = &output.terminal {
                        return Err(StreamWriteError::Closed(cause.clone()));
                    }
                    output.terminal = Some(ByteStreamCloseCause::Failed(failure));
                    Ok(())
                });
                Ok((result,))
            })
        },
    )?;

    let mut host = linker.instance("golem:tool/host@0.1.0")?;
    for name in [
        "tool-stdin-writer",
        "tool-stdin",
        "tool-stdin-closed",
        "tool-stdout",
        "tool-rpc",
        "future-invoke-result",
    ] {
        host.resource(name, ResourceType::host::<UnusedResource>(), |_store, _| {
            Ok(())
        })?;
    }
    let tool_host = component
        .component_type()
        .imports(&engine)
        .find_map(|(name, item)| (name == "golem:tool/host@0.1.0").then_some(item.ty));
    let Some(ComponentItem::ComponentInstance(tool_host)) = tool_host else {
        bail!("fixture does not import golem:tool/host");
    };
    for (name, item) in tool_host.exports(&engine) {
        if name.starts_with("[method]tool-stdout-writer.") {
            continue;
        }
        match item.ty {
            ComponentItem::ComponentFunc(function) if function.async_() => {
                host.func_new_concurrent(name, |_accessor, _, _, _| {
                    Box::pin(async { Err(wasmtime::Error::msg("unexpected tool host call")) })
                })?;
            }
            ComponentItem::ComponentFunc(_) => {
                host.func_new(name, |_store, _, _, _| {
                    Err(wasmtime::Error::msg("unexpected tool host call"))
                })?;
            }
            _ => {}
        }
    }
    for (interface_name, item) in component.component_type().imports(&engine) {
        let ComponentItem::ComponentInstance(interface) = item.ty else {
            continue;
        };
        if matches!(
            interface_name,
            "golem:tool/host@0.1.0" | "golem:tool/streams@0.1.0"
        ) {
            continue;
        }
        let mut linker_interface = linker.instance(interface_name)?;
        for (name, item) in interface.exports(&engine) {
            match item.ty {
                ComponentItem::ComponentFunc(function) if function.async_() => {
                    linker_interface.func_new_concurrent(name, |_accessor, _, _, _| {
                        Box::pin(async { Err(wasmtime::Error::msg("unexpected fixture import")) })
                    })?;
                }
                ComponentItem::ComponentFunc(_) => {
                    linker_interface.func_new(name, |_store, _, _, _| {
                        Err(wasmtime::Error::msg("unexpected fixture import"))
                    })?;
                }
                ComponentItem::Resource(_) => {
                    let ty = match name {
                        "quota-token" => ResourceType::host::<QuotaTokenHandleRep>(),
                        "secret" => ResourceType::host::<SecretHandleRep>(),
                        "permission-card" => ResourceType::host::<PermissionCardHandleRep>(),
                        "schema-value-stream" => ResourceType::host::<SchemaValueStreamHandleRep>(),
                        _ => ResourceType::host::<UnusedResource>(),
                    };
                    linker_interface.resource(name, ty, |_store, _| Ok(()))?;
                }
                _ => {}
            }
        }
    }
    let mut store = Store::new(&engine, Host::default());
    store.set_fuel(u64::MAX)?;
    store.set_epoch_deadline(u64::MAX);
    let instance = linker.instantiate_async(&mut store, &component).await?;
    Ok((store, instance))
}

fn new_stdout(store: &mut Store<Host>) -> (Resource<StdoutWriter>, Arc<Mutex<OutputState>>) {
    let rep = store.data().next_writer;
    store.data_mut().next_writer += 1;
    let reader = Arc::new(Mutex::new(OutputState::default()));
    store.data_mut().writers.insert(rep, reader.clone());
    (Resource::new_own(rep), reader)
}

fn input(
    tool: &Tool,
    command: &str,
    fields: Vec<SchemaValue>,
) -> Result<schema_wire::TypedSchemaValue> {
    let index = tool
        .command_index_by_path(&[command.to_string()])
        .context("command metadata")?;
    let graph = tool.canonical_input_record_schema(index)?;
    Ok(encode_typed(&TypedSchemaValue::new(
        graph,
        SchemaValue::Record { fields },
    ))?)
}

fn principal() -> agent_wire::Principal {
    Principal::anonymous().into()
}

#[test]
#[timeout("60s")]
async fn moonbit_tool_guest_exports_stream_and_reject_invalid_calls(
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
) -> Result<()> {
    let path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let (mut store, instance) = instantiate(&path).await?;
    let discover = exported(&mut store, &instance, "discover-tools")?
        .typed::<(), (Result<Vec<tool_wire::Tool>, tool_wire::ToolError>,)>(&store)?;
    let tools = discover
        .call_async(&mut store, ())
        .await?
        .0
        .map_err(|error| anyhow::anyhow!("discover-tools rejected: {error:?}"))?;
    if tools.len() != 1 {
        bail!("expected one MoonBit tool, got {}", tools.len());
    }
    let native_tool = Tool::try_from(&tools[0])?;

    let get = exported(&mut store, &instance, "get-tool")?
        .typed::<(String,), (Result<tool_wire::Tool, tool_wire::ToolError>,)>(&store)?;
    let found = get
        .call_async(&mut store, ("moonbit-streaming".into(),))
        .await?
        .0
        .map_err(|error| anyhow::anyhow!("get-tool rejected known tool: {error:?}"))?;
    assert_eq!(Tool::try_from(&found)?, native_tool);
    assert!(matches!(
        get.call_async(&mut store, ("missing".into(),)).await?.0,
        Err(tool_wire::ToolError::InvalidToolName(_))
    ));

    let invoke = exported(&mut store, &instance, "invoke")?.typed::<(
        String,
        Vec<String>,
        schema_wire::TypedSchemaValue,
        Option<wasmtime::component::StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
        Option<Resource<StdoutWriter>>,
        agent_wire::Principal,
    ), (Invocation,)>(&store)?;
    for (command, expected_bytes, expected_value) in [
        ("stream", b"moonbit-marker:\x00\xffABC\x80".as_slice(), 6),
        ("stdout-only", b"stdout-only:\x00\xff".as_slice(), 73),
        ("no-streams", b"".as_slice(), 91),
    ] {
        let (writer, reader) = if command == "no-streams" {
            (None, None)
        } else {
            let (writer, reader) = new_stdout(&mut store);
            (Some(writer), Some(reader))
        };
        let (stdin, fields) = if command == "stream" {
            (
                Some(StreamReader::new(
                    &mut store,
                    vec![Ok(vec![0, 255]), Ok(vec![65, 66, 67, 128])],
                )?),
                vec![SchemaValue::String("marker-echo".into())],
            )
        } else {
            (None, vec![])
        };
        let result = invoke
            .call_async(
                &mut store,
                (
                    "moonbit-streaming".into(),
                    vec![command.into()],
                    input(&native_tool, command, fields)?,
                    stdin,
                    writer,
                    principal(),
                ),
            )
            .await?
            .0
            .map_err(|error| anyhow::anyhow!("invoke {command} rejected: {error:?}"))?;
        assert!(
            result.stdout.is_none(),
            "stdout must use only the supplied writer"
        );
        assert_eq!(
            decode_value(&result.result.context("structured result")?.value)?,
            SchemaValue::U64(expected_value)
        );
        if let Some(reader) = reader {
            let output = reader.lock().unwrap();
            assert_eq!(output.bytes, expected_bytes);
            assert_eq!(output.terminal, Some(ByteStreamCloseCause::Finished));
            assert_eq!(output.drops, 1);
        }
        assert!(store.data().writers.is_empty());
    }

    for (tool, command, stdout, stdin, invalid_input, expected) in [
        ("missing", "stream", true, true, false, "tool"),
        ("moonbit-streaming", "missing", true, true, false, "path"),
        ("moonbit-streaming", "stream", false, true, false, "stdout"),
        (
            "moonbit-streaming",
            "no-streams",
            true,
            true,
            false,
            "unexpected",
        ),
        ("moonbit-streaming", "stream", true, false, false, "stdin"),
        ("moonbit-streaming", "stream", true, true, true, "input"),
    ] {
        let (writer, reader) = if stdout {
            let (writer, reader) = new_stdout(&mut store);
            (Some(writer), Some(reader))
        } else {
            (None, None)
        };
        let stdin_drops = Arc::new(AtomicUsize::new(0));
        let attachment = if stdin {
            Some(StreamReader::new(
                &mut store,
                PendingInput(stdin_drops.clone()),
            )?)
        } else {
            None
        };
        let fields = if invalid_input {
            vec![]
        } else {
            vec![SchemaValue::String("marker-echo".into())]
        };
        let rejected = invoke
            .call_async(
                &mut store,
                (
                    tool.into(),
                    vec![command.into()],
                    input(&native_tool, "stream", fields)?,
                    attachment,
                    writer,
                    principal(),
                ),
            )
            .await?
            .0;
        match (expected, rejected) {
            ("tool", Err(tool_wire::ToolError::InvalidToolName(name))) => {
                assert_eq!(name, "missing")
            }
            ("path", Err(tool_wire::ToolError::InvalidCommandPath(path))) => {
                assert_eq!(path, ["missing"])
            }
            ("stdout", Err(tool_wire::ToolError::InvalidInput(message))) => {
                assert_eq!(message, "required stdout stream is missing")
            }
            ("unexpected", Err(tool_wire::ToolError::InvalidInput(message))) => {
                assert_eq!(message, "command does not accept a stdout stream")
            }
            ("stdin", Err(tool_wire::ToolError::InvalidInput(message))) => {
                assert!(message.contains("stdin"), "{message}")
            }
            ("input", Err(tool_wire::ToolError::InvalidInput(_))) => (),
            (expected, actual) => bail!("expected {expected} rejection, got {actual:?}"),
        }
        assert_eq!(stdin_drops.load(Ordering::SeqCst), usize::from(stdin));
        if let Some(reader) = reader {
            let output = reader.lock().unwrap();
            assert!(output.bytes.is_empty());
            assert!(matches!(
                output.terminal,
                Some(ByteStreamCloseCause::Failed(_))
            ));
            assert_eq!(output.drops, 1);
        }
        assert!(store.data().writers.is_empty());
    }
    Ok(())
}
