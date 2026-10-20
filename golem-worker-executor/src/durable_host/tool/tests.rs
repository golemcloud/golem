// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{
    ResolvedToolCommand, SkippedToolAttachmentEndpoints, ToolOutputEntry, ToolOutputWriterEntry,
    ToolStdinEntry, ToolStdinStreamConsumer, UnderlyingToolEntry, WitRegisteredTool,
    await_native_entity_body, caller_tool_owner, classify_tool_discovery_error,
    cleanup_tool_endpoints, first_output_limit_error, guest_body_accepts_admission_rejection,
    materialize_output_writer, merge_discovered_tools, native_output_handles,
    native_output_runtime_teardown_probe, option_args, output_limit_error,
    publish_output_completions, recorded_tool_body_is_skipped, resolve_tool_command,
    select_native_body_result, terminal_tool_discovery_error, validate_declared_tool_error,
    validate_declared_tool_result, validate_native_tool_output, validate_stream_attachments,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::entity::RecordedEntityTerminal;
use crate::durable_host::stream_bus::{LiveStreamEventPayload, LiveStreamReceiveError};
use crate::durable_host::stream_transport::{
    LiveStreamEndpoint, accounted_byte_output_stream_pair,
};
use crate::durable_host::tool::attachment::{
    AttachmentMemory, AttachmentRead, ToolAttachmentModeMetadata, attachment_pair,
};
use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};

use crate::preview2::golem::tool::host::{ByteStreamCloseCause, ByteStreamFailure};
use crate::services::environment_state::ToolDiscoveryError;
use futures::FutureExt;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::ToolOwnerPattern;
use golem_common::model::card::{ToolArgPattern, ToolIdentifier, ToolInvocationValue};
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{EntityCallMode, NamedToolErrorSchema, ToolOutputContract};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::oplog::HostResponseEntityInvocation;
use golem_common::model::oplog::payload::types::{
    SerializableCustomToolError, SerializableEntityBodyExecution, SerializableToolError,
    SerializableToolOperationTerminal, SerializableToolResultValue, SerializableToolRpcError,
    SerializableToolStructuredResult,
};
use golem_common::model::oplog::payload::types::{
    SerializableDiscoveredTools, SerializableMcpImportDiscovery, SerializableToolDiscoverySnapshot,
};
use golem_common::model::tool::{
    RegisteredTool, ToolBindingInput, ToolBindingOwner, ToolDeploymentState, ToolName,
    ToolProvisionConfig, ToolSource,
};
use golem_common::model::tool_middleware::{
    RegisteredToolMiddleware, ToolMiddlewareInstallation, ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::schema::tool::{
    CommandBody, CommandNode, CommandTree, Constraint, DiscoveredTool, Doc, Globals,
    MonomorphicToolMiddlewareScope, OptionShape, OptionSpec, Positional, Positionals, Ref,
    Repetition, Tool, ToolMiddleware, ToolMiddlewareScope,
};
use golem_common::schema::{
    IntoTypedSchemaValue, MetadataEnvelope, SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue,
    TypeId, TypedSchemaValue,
};
use golem_schema::schema::SchemaValueStream;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use test_r::test;
use test_r::timeout;
use tokio::sync::mpsc;
use wasmtime::component::{Component, Linker, Resource, ResourceTable, StreamReader};
use wasmtime::{Config, Engine, Store};

#[test]
fn underlying_capability_rejects_missing_and_wrong_type_resource_handles() {
    let mut table = ResourceTable::new();
    let missing = Resource::<UnderlyingToolEntry>::new_borrow(42);
    assert!(table.get(&missing).is_err());

    let occupied = table
        .push("not an underlying capability".to_string())
        .unwrap();
    let wrong_type = Resource::<UnderlyingToolEntry>::new_borrow(occupied.rep());
    assert!(table.get(&wrong_type).is_err());
    assert_eq!(
        table.get(&occupied).unwrap(),
        "not an underlying capability",
        "a failed forged lookup must not consume or replace the actual table entry"
    );
}

#[test]
#[timeout("5s")]
async fn native_output_owner_failure_aborts_without_publishing_end() {
    let owner = OwnerToolOperations::new();
    let primary_teardown = Arc::new(|| false);
    let runtime_teardown =
        native_output_runtime_teardown_probe(primary_teardown.clone(), Arc::downgrade(&owner));
    assert!(!runtime_teardown());
    let (sink, stream) = accounted_byte_output_stream_pair(
        4,
        runtime_teardown.clone(),
        None,
        Box::pin(std::future::pending()),
    )
    .unwrap();
    let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
    let lifecycle = endpoint.lifecycle();
    let mut primary = endpoint.activate();

    assert!(
        owner
            .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
            .await
    );
    assert!(!primary_teardown());
    assert!(runtime_teardown());
    drop(sink);

    lifecycle.cancelled().await;
    assert!(lifecycle.is_aborted());
    assert!(matches!(
        primary.recv().now_or_never(),
        None | Some(Err(LiveStreamReceiveError::Closed))
    ));
}

#[test]
#[timeout("5s")]
async fn native_output_unfenced_eof_preserves_bytes_and_end() {
    let owner = OwnerToolOperations::new();
    let runtime_teardown =
        native_output_runtime_teardown_probe(Arc::new(|| false), Arc::downgrade(&owner));
    assert!(!runtime_teardown());
    let (producer, consumer, _) = attachment_pair(8, AttachmentMemory::inert());
    assert!(producer.configure_live());
    producer.write(vec![17, 255]).await.unwrap();
    producer.finish().unwrap();

    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let reader = StreamReader::new(&mut store, consumer.into_raw_stream_producer()).unwrap();
    let (sink, stream) = accounted_byte_output_stream_pair(
        4,
        runtime_teardown,
        None,
        Box::pin(std::future::pending()),
    )
    .unwrap();
    let endpoint = stream.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
    let lifecycle = endpoint.lifecycle();
    let mut primary = endpoint.activate();
    let bytes = store
        .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<u8>> {
            accessor.with(|mut store| reader.pipe(&mut store, sink))?;
            let mut bytes = Vec::new();
            loop {
                let event = primary.recv().await.unwrap();
                assert_eq!(event.offset, bytes.len() as u64);
                match event.payload {
                    LiveStreamEventPayload::Item(SchemaValue::U8(byte)) => bytes.push(byte),
                    LiveStreamEventPayload::End => break,
                    other => panic!("unexpected native output event: {other:?}"),
                }
            }
            Ok(bytes)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, vec![17, 255]);
    assert!(!lifecycle.is_aborted());
    assert!(owner.selected_owner_failure().is_none());
}

#[test]
fn native_output_teardown_probe_preserves_primary_without_retaining_owner() {
    let owner = OwnerToolOperations::new();
    let weak_owner = Arc::downgrade(&owner);
    let primary_teardown = Arc::new(AtomicBool::new(false));
    let primary_flag = primary_teardown.clone();
    let runtime_teardown = native_output_runtime_teardown_probe(
        Arc::new(move || primary_flag.load(Ordering::Acquire)),
        weak_owner.clone(),
    );
    assert_eq!(Arc::strong_count(&owner), 1);
    assert!(!runtime_teardown());
    assert_eq!(Arc::strong_count(&owner), 1);

    drop(owner);
    assert!(weak_owner.upgrade().is_none());
    assert!(!runtime_teardown());
    primary_teardown.store(true, Ordering::Release);
    assert!(runtime_teardown());
    assert!(weak_owner.upgrade().is_none());
}

#[test]
#[timeout("10s")]
async fn pending_native_entity_setup_is_cancelled_by_runner_abort() {
    let runner_abort = tokio_util::sync::CancellationToken::new();
    let setup_polled = Arc::new(tokio::sync::Notify::new());
    let setup_polled_by_future = setup_polled.clone();
    let abort = runner_abort.clone();
    let cancel = tokio::spawn(async move {
        setup_polled.notified().await;
        abort.cancel();
    });
    let setup = std::future::poll_fn(move |_| {
        setup_polled_by_future.notify_one();
        Poll::<Result<(), WorkerExecutorError>>::Pending
    });

    let outcome = await_native_entity_body(&runner_abort, None, Default::default(), setup)
        .await
        .unwrap();
    cancel.await.unwrap();

    assert!(matches!(outcome, super::EntityBodyOutcome::Cancelled));
}

#[test]
async fn pre_cancelled_runner_abort_does_not_poll_native_handler() {
    let runner_abort = tokio_util::sync::CancellationToken::new();
    runner_abort.cancel();
    let handler_polls = Arc::new(AtomicUsize::new(0));
    let handler_polls_by_future = handler_polls.clone();
    let handler = std::future::poll_fn(move |_| {
        handler_polls_by_future.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(Ok::<_, WorkerExecutorError>(()))
    });

    let outcome = await_native_entity_body(&runner_abort, None, Default::default(), handler)
        .await
        .unwrap();

    assert!(matches!(outcome, super::EntityBodyOutcome::Cancelled));
    assert_eq!(handler_polls.load(Ordering::SeqCst), 0);
}

#[test]
async fn started_native_failure_precedes_simultaneous_runner_abort() {
    let runner_abort = tokio_util::sync::CancellationToken::new();
    let (send, receive) = tokio::sync::oneshot::channel();
    let expected = WorkerExecutorError::runtime("reconstruction failed");
    let body = async { receive.await.unwrap() };
    let mut run = Box::pin(await_native_entity_body(
        &runner_abort,
        None,
        Default::default(),
        body,
    ));
    assert!(futures::poll!(run.as_mut()).is_pending());
    send.send(Err::<(), _>(expected.clone())).unwrap();
    runner_abort.cancel();
    assert_eq!(run.await.unwrap_err(), expected);
}

#[test]
async fn observed_native_failure_survives_abort_while_election_is_pending() {
    let abort = tokio_util::sync::CancellationToken::new();
    let finalization = super::EntityBodyFinalization::default();
    let body_finalization = finalization.clone();
    let (release, gate) = tokio::sync::oneshot::channel();
    let expected = WorkerExecutorError::recovery_required("native host failed");
    let failure = expected.clone();
    let body = async move {
        body_finalization.begin();
        gate.await.unwrap();
        Err::<(), _>(failure)
    };
    let mut run = Box::pin(await_native_entity_body(&abort, None, finalization, body));
    assert!(futures::poll!(run.as_mut()).is_pending());
    abort.cancel();
    assert!(futures::poll!(run.as_mut()).is_pending());
    assert!(!release.is_closed());
    release.send(()).unwrap();
    assert_eq!(run.await.unwrap_err(), expected);
}

#[test]
fn completed_guest_admission_refusal_preserves_unrelated_body_failures() {
    use crate::worker::invocation::GuestCallSettlementError;

    assert!(guest_body_accepts_admission_rejection(&Ok(Ok(()))));
    let refusal = crate::durable_host::tool_attachment_live_admission_rejected_error();
    assert!(guest_body_accepts_admission_rejection::<()>(&Ok(Err(
        wasmtime::Error::new(refusal).context("host call failed"),
    ))));
    for result in [
        Ok(Err(wasmtime::Error::msg("unrelated guest failure"))),
        Err(GuestCallSettlementError::Trap(wasmtime::Error::msg(
            "unrelated trap",
        ))),
        Err(GuestCallSettlementError::Interrupted(wasmtime::Error::msg(
            "interrupted",
        ))),
        Err(GuestCallSettlementError::Infrastructure(
            WorkerExecutorError::runtime("unrelated infrastructure failure"),
        )),
    ] {
        assert!(!guest_body_accepts_admission_rejection::<()>(&result));
    }
}

#[test]
fn native_post_body_admission_rejection_overrides_a_caught_handler_error() {
    let result =
        select_native_body_result(Ok("handler returned success"), Ok(()), true).unwrap_err();

    assert!(crate::durable_host::is_tool_attachment_live_admission_rejection(&result));
}

#[test]
fn native_body_failure_precedes_parent_cleanup_failure() {
    let result = select_native_body_result::<()>(
        Err(WorkerExecutorError::runtime("body failed")),
        Err(WorkerExecutorError::runtime("cleanup failed")),
        true,
    )
    .unwrap_err();

    assert!(format!("{result}").contains("body failed"));
}

#[test]
fn native_parent_cleanup_failure_replaces_a_successful_declared_result() {
    let declared_error = SerializableToolRpcError::RemoteToolError(Box::new(
        SerializableToolError::InvalidResult("declared failure".to_string()),
    ));
    let result = select_native_body_result(
        Ok(Err::<SerializableToolStructuredResult, _>(declared_error)),
        Err(WorkerExecutorError::runtime("cleanup failed")),
        false,
    )
    .unwrap_err();

    assert!(format!("{result}").contains("cleanup failed"));
}

#[test]
async fn actual_host_output_exhaustion_precedes_a_declared_tool_error_for_each_channel() {
    for (exhaust_stdout, expected_channel) in [(true, "stdout"), (false, "stderr")] {
        let (stdout, _stdout_reader, stdout_observer) =
            attachment_pair(2, AttachmentMemory::inert());
        let (stderr, _stderr_reader, stderr_observer) =
            attachment_pair(2, AttachmentMemory::inert());
        assert!(stdout.configure_completion());
        assert!(stderr.configure_completion());
        let exhausted = if exhaust_stdout { &stdout } else { &stderr };
        assert!(matches!(
            exhausted.write(vec![1, 2, 3]).await,
            Err(
                crate::preview2::golem::tool::host::StreamWriteError::Closed(
                    ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
                )
            )
        ));

        let selected = first_output_limit_error(Some(&stdout_observer), Some(&stderr_observer))
            .unwrap_or_else(|| {
                SerializableToolRpcError::RemoteToolError(Box::new(
                    SerializableToolError::InvalidResult("declared failure".to_string()),
                ))
            });
        assert!(matches!(
            selected,
            SerializableToolRpcError::ResourceExhausted(message)
                if message.contains(expected_channel)
        ));
    }
    assert!(output_limit_error("stderr", false).is_none());
}

fn typed_wasmtime_writer_component(engine: &Engine) -> Component {
    Component::new(
        engine,
        r#"
(component
  (import "attach" (func $attach))
  (core func $attach (canon lower (func $attach)))
  (core module $memory
    (memory (export "mem") 1)
    (data (i32.const 128) "\01\02\03\00\04\05\06\00\07\08\09\00\0a\0b\0c")
  )
  (core instance $memory (instantiate $memory))
  (core module $core
    (import "" "mem" (memory 1))
    (import "" "stream.new" (func $stream.new (result i64)))
    (import "" "stream.write-async" (func $stream.write-async (param i32 i32 i32) (result i32)))
    (import "" "stream.write-sync" (func $stream.write-sync (param i32 i32 i32) (result i32)))
    (import "" "stream.cancel-write" (func $stream.cancel-write (param i32) (result i32)))
    (import "" "stream.drop-writable" (func $stream.drop-writable (param i32)))
    (import "" "attach" (func $attach))
    (global $writer (mut i32) (i32.const 0))
    (func $set-item (param $item i32) (param $bytes i32)
      (i32.store8 (local.get $item) (i32.const 0))
      (i32.store (i32.add (local.get $item) (i32.const 4)) (local.get $bytes))
      (i32.store (i32.add (local.get $item) (i32.const 8)) (i32.const 3))
    )
    (func (export "start") (result i32)
      (local $pair i64)
      (call $set-item (i32.const 0) (i32.const 128))
      (call $set-item (i32.const 16) (i32.const 132))
      (call $set-item (i32.const 32) (i32.const 136))
      (call $set-item (i32.const 48) (i32.const 140))
      (local.set $pair (call $stream.new))
      (global.set $writer (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
      (i32.wrap_i64 (local.get $pair))
    )
    (func (export "cancel-without-acknowledgement")
      (local $result i32)
      (local.set $result (call $stream.write-async (global.get $writer) (i32.const 48) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (call $attach)
      (local.set $result (call $stream.cancel-write (global.get $writer)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
    )
    (func (export "attach") (call $attach))
    (func (export "write-first")
      (local $result i32)
      (local.set $result (call $stream.write-sync (global.get $writer) (i32.const 0) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const 16)) (then unreachable))
    )
    (func (export "cancel-with-pending-acknowledgement")
      (local $result i32)
      (local.set $result (call $stream.write-async (global.get $writer) (i32.const 16) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const -1)) (then unreachable))
      (local.set $result (call $stream.cancel-write (global.get $writer)))
      (if (i32.ne (local.get $result) (i32.const 2)) (then unreachable))
    )
    (func (export "resume-and-close")
      (local $result i32)
      (local.set $result (call $stream.write-sync (global.get $writer) (i32.const 32) (i32.const 1)))
      (if (i32.ne (local.get $result) (i32.const 16)) (then unreachable))
      (call $stream.drop-writable (global.get $writer))
    )
  )
  (type $failure (variant
    (case "cancelled")
    (case "abandoned")
    (case "resource-exhausted")
    (case "failed" string)
  ))
  (export $failure-export "byte-stream-failure" (type $failure))
  (type $item (result (list u8) (error $failure-export)))
  (export $item-export "byte-stream-item" (type $item))
  (type $stream (stream $item-export))
  (core func $stream.new (canon stream.new $stream))
  (core func $stream.write-async (canon stream.write $stream async (memory $memory "mem")))
  (core func $stream.write-sync (canon stream.write $stream (memory $memory "mem")))
  (core func $stream.cancel-write (canon stream.cancel-write $stream))
  (core func $stream.drop-writable (canon stream.drop-writable $stream))
  (core instance $core (instantiate $core (with "" (instance
    (export "mem" (memory $memory "mem"))
    (export "stream.new" (func $stream.new))
    (export "stream.write-async" (func $stream.write-async))
    (export "stream.write-sync" (func $stream.write-sync))
    (export "stream.cancel-write" (func $stream.cancel-write))
    (export "stream.drop-writable" (func $stream.drop-writable))
    (export "attach" (func $attach))
  ))))
  (func (export "start") async (result (stream $item-export))
    (canon lift (core func $core "start")))
  (func (export "cancel-without-acknowledgement") async
    (canon lift (core func $core "cancel-without-acknowledgement")))
  (func (export "attach") async
    (canon lift (core func $core "attach")))
  (func (export "write-first") async
    (canon lift (core func $core "write-first")))
  (func (export "cancel-with-pending-acknowledgement") async
    (canon lift (core func $core "cancel-with-pending-acknowledgement")))
  (func (export "resume-and-close") async
    (canon lift (core func $core "resume-and-close")))
)
            "#,
    )
    .unwrap()
}

async fn assert_typed_stdin_operation_cancellation(pending_acknowledgement: bool) {
    let (_attachment_producer, _attachment_consumer, observer) =
        attachment_pair(3, AttachmentMemory::inert());
    let (items, mut received) = mpsc::unbounded_channel();
    let mut config = Config::new();
    config.concurrency_support(true);
    config.wasm_component_model_more_async_builtins(true);
    let engine = Engine::new(&config).unwrap();
    let component = typed_wasmtime_writer_component(&engine);
    let mut store = Store::new(&engine, ());
    let reader_slot = std::sync::Arc::new(std::sync::Mutex::new(
        None::<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
    ));
    let reader_slot_for_attach = reader_slot.clone();
    let observer_during_cancellation = observer.clone();
    let mut linker = Linker::new(&engine);
    linker
        .root()
        .func_wrap("attach", move |mut store, (): ()| {
            reader_slot_for_attach
                .lock()
                .unwrap()
                .take()
                .expect("typed stream reader was not ready to attach")
                .pipe(
                    &mut store,
                    ToolStdinStreamConsumer::new(items.clone(), observer.clone()),
                )?;
            Ok(())
        })
        .unwrap();
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let start = instance
        .get_typed_func::<(), (StreamReader<Result<Vec<u8>, ByteStreamFailure>>,)>(
            &mut store, "start",
        )
        .unwrap();
    let attach = instance
        .get_typed_func::<(), ()>(&mut store, "attach")
        .unwrap();
    let write_first = instance
        .get_typed_func::<(), ()>(&mut store, "write-first")
        .unwrap();
    let cancel = instance
        .get_typed_func::<(), ()>(
            &mut store,
            if pending_acknowledgement {
                "cancel-with-pending-acknowledgement"
            } else {
                "cancel-without-acknowledgement"
            },
        )
        .unwrap();
    let resume = instance
        .get_typed_func::<(), ()>(&mut store, "resume-and-close")
        .unwrap();

    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            let (reader,) = start.call_concurrent(accessor, ()).await?;
            *reader_slot.lock().unwrap() = Some(reader);
            let first = if pending_acknowledgement {
                attach.call_concurrent(accessor, ()).await?;
                write_first.call_concurrent(accessor, ()).await?;
                let first = received.recv().await.unwrap();
                assert_eq!(first.item.as_ref().unwrap(), &vec![1, 2, 3]);
                Some(first)
            } else {
                None
            };
            cancel.call_concurrent(accessor, ()).await?;
            assert!(received.try_recv().is_err());
            assert!(observer_during_cancellation.terminal_snapshot().is_none());
            if let Some(first) = first {
                first.acknowledged.send(()).unwrap();
            }

            resume.call_concurrent(accessor, ()).await?;
            let resumed = received.recv().await.unwrap();
            assert_eq!(resumed.item.unwrap(), vec![7, 8, 9]);
            let _ = resumed.acknowledged.send(());
            assert!(received.try_recv().is_err());
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
}

#[test]
async fn cancelling_typed_stdin_without_acknowledgement_resumes_without_false_eof() {
    assert_typed_stdin_operation_cancellation(false).await;
}

#[test]
async fn cancelling_typed_stdin_with_pending_acknowledgement_preserves_it_and_resumes() {
    assert_typed_stdin_operation_cancellation(true).await;
}

#[test]
fn failed_dispatch_attempts_all_endpoint_cleanups_and_preserves_the_original_error() {
    let attempted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stdin_attempted = attempted.clone();
    let stdout_attempted = attempted.clone();
    let stderr_attempted = attempted.clone();
    let error = cleanup_tool_endpoints(
        anyhow::anyhow!("dispatch failed"),
        move || {
            stdin_attempted.lock().unwrap().push("stdin");
            Err(anyhow::anyhow!("missing stdin"))
        },
        move || {
            stdout_attempted.lock().unwrap().push("stdout");
            Err(anyhow::anyhow!("missing stdout"))
        },
        move || {
            stderr_attempted.lock().unwrap().push("stderr");
            Err(anyhow::anyhow!("missing stderr"))
        },
    );

    assert_eq!(
        *attempted.lock().unwrap(),
        vec!["stdin", "stdout", "stderr"]
    );
    let message = format!("{error:#}");
    assert!(message.contains("dispatch failed"));
    assert!(message.contains("stdin cleanup failed: missing stdin"));
    assert!(message.contains("stdout cleanup failed: missing stdout"));
    assert!(message.contains("stderr cleanup failed: missing stderr"));
}

#[test]
async fn production_output_wiring_keeps_stderr_only_and_dual_payloads_distinct() {
    let (stdout_producer, stdout_reader, _) = attachment_pair(16, AttachmentMemory::inert());
    let (stderr_producer, stderr_reader, _) = attachment_pair(16, AttachmentMemory::inert());
    let stdout = materialize_output_writer(
        Some(ToolOutputEntry {
            producer: Some(stdout_producer),
            completion_only: false,
        }),
        false,
        AttachmentMemory::inert,
    )
    .unwrap();
    let stderr = materialize_output_writer(
        Some(ToolOutputEntry {
            producer: Some(stderr_producer),
            completion_only: false,
        }),
        false,
        AttachmentMemory::inert,
    )
    .unwrap();
    assert!(stdout.controller().configure_live());
    assert!(stderr.controller().configure_live());

    let tasks = crate::worker::tasks::NativeReconstructionTasks::new(
        &crate::worker::tasks::WorkerTasks::default(),
    )
    .unwrap();
    let spawner = tasks.spawner();
    let (stdout_handle, stderr_handle) =
        native_output_handles(Some(&stdout), Some(&stderr), &spawner);
    let stdout_handle = stdout_handle.unwrap();
    let stderr_handle = stderr_handle.unwrap();
    stdout_handle.writer.write(vec![0, 1, 2]).await.unwrap();
    stderr_handle.writer.write(vec![9, 8]).await.unwrap();
    stdout_handle.writer.finish().unwrap();
    stderr_handle.writer.finish().unwrap();
    assert!(matches!(
        stdout_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![0, 1, 2]
    ));
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![9, 8]
    ));

    let (stderr_producer, stderr_reader, _) = attachment_pair(8, AttachmentMemory::inert());
    let stderr = ToolOutputEntry {
        producer: Some(stderr_producer),
        completion_only: false,
    }
    .into_writer();
    assert!(stderr.controller().configure_live());
    let (stdout_handle, stderr_handle) = native_output_handles(None, Some(&stderr), &spawner);
    assert!(stdout_handle.is_none());
    let stderr_handle = stderr_handle.unwrap();
    stderr_handle.writer.write(vec![7, 6]).await.unwrap();
    stderr_handle.writer.finish().unwrap();
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![7, 6]
    ));
}

#[test]
async fn output_policy_omits_optional_writers_and_discards_declared_fire_and_forget_output() {
    let optional = materialize_output_writer(None, false, || {
        panic!("an omitted optional output must not allocate a discard attachment")
    });
    assert!(optional.is_none());

    let discard = materialize_output_writer(None, true, AttachmentMemory::inert).unwrap();
    let controller = discard.controller();
    assert_eq!(
        controller.metadata().mode,
        ToolAttachmentModeMetadata::Discard
    );
    let tasks = crate::worker::tasks::NativeReconstructionTasks::new(
        &crate::worker::tasks::WorkerTasks::default(),
    )
    .unwrap();
    let output = discard.native_writer(tasks.spawner());
    output.writer.write(vec![5; 1024 * 1024]).await.unwrap();
    output.writer.finish().unwrap();
}

#[test]
async fn staged_output_publication_preserves_independent_terminals() {
    let (stdout, stdout_reader, _) = attachment_pair(8, AttachmentMemory::inert());
    let (stderr, stderr_reader, _) = attachment_pair(8, AttachmentMemory::inert());
    let stdout_controller = stdout.controller();
    let stderr_controller = stderr.controller();
    assert!(stdout_controller.configure_completion());
    assert!(stderr_controller.configure_completion());
    stdout.write(vec![1, 2]).await.unwrap();
    stderr.write(vec![9]).await.unwrap();
    stdout.finish().unwrap();
    stderr_controller
        .host_fail(ByteStreamFailure::Failed("stderr failed".to_string()))
        .unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            stdout_reader.read_next()
        )
        .await
        .is_err()
    );
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            stderr_reader.read_next()
        )
        .await
        .is_err()
    );

    publish_output_completions(
        true,
        false,
        false,
        Some(&stdout_controller),
        Some(&stderr_controller),
    );
    assert!(matches!(
        stdout_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![1, 2]
    ));
    assert!(matches!(
        stdout_reader.read_next().await,
        AttachmentRead::End
    ));
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Ok(bytes)) if bytes == vec![9]
    ));
    assert!(matches!(
        stderr_reader.read_next().await,
        AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason)))
            if reason == "stream producer failed"
    ));
}

#[test]
async fn partial_dispatch_cleanup_rejects_both_real_output_endpoints() {
    let (stdout, stdout_reader, _) = attachment_pair(8, AttachmentMemory::inert());
    let (stderr, stderr_reader, _) = attachment_pair(8, AttachmentMemory::inert());
    let mut stdout = Some(ToolOutputEntry {
        producer: Some(stdout),
        completion_only: false,
    });
    let mut stderr = Some(ToolOutputEntry {
        producer: Some(stderr),
        completion_only: false,
    });

    let error = cleanup_tool_endpoints(
        anyhow::anyhow!("stderr setup failed"),
        || Ok(()),
        || {
            stdout
                .take()
                .unwrap()
                .producer
                .as_ref()
                .unwrap()
                .reject_unconfigured()
                .unwrap();
            Ok(())
        },
        || {
            stderr
                .take()
                .unwrap()
                .producer
                .as_ref()
                .unwrap()
                .reject_unconfigured()
                .unwrap();
            Ok(())
        },
    );
    assert_eq!(format!("{error:#}"), "stderr setup failed");
    assert!(stdout.is_none());
    assert!(stderr.is_none());
    for reader in [stdout_reader, stderr_reader] {
        assert!(matches!(
            reader.read_next().await,
            AttachmentRead::Item(Err(ByteStreamFailure::Failed(reason)))
                if reason == "tool invocation rejected"
        ));
    }
}

#[test]
fn completed_skipped_terminal_selects_no_body_replay() {
    let response = HostResponseEntityInvocation {
        result: Ok(SerializableToolOperationTerminal {
            admission_refusal: None,
            body_execution: SerializableEntityBodyExecution::Skipped,
            result: Err(SerializableToolRpcError::ResourceExhausted(
                "recorded limit".to_string(),
            )),
        }
        .into_typed_schema_value()
        .unwrap()),
    };
    assert!(recorded_tool_body_is_skipped(&RecordedEntityTerminal::Completed(response)).unwrap());
}

#[test]
fn skipped_replay_publishes_recorded_failure_before_endpoint_roles_drop() {
    for failure in [
        ByteStreamFailure::ResourceExhausted,
        ByteStreamFailure::Cancelled,
    ] {
        let (stdin_producer, stdin_consumer, stdin_observer) =
            attachment_pair(16, AttachmentMemory::inert());
        let (stdout_producer, stdout_consumer, stdout_observer) =
            attachment_pair(16, AttachmentMemory::inert());
        let (stderr_producer, stderr_consumer, stderr_observer) =
            attachment_pair(16, AttachmentMemory::inert());
        let endpoints = SkippedToolAttachmentEndpoints {
            stdin: Some(ToolStdinEntry {
                consumer: stdin_consumer,
            }),
            stdout: Some(ToolOutputWriterEntry {
                producer: stdout_producer,
                completion_only: false,
            }),
            stderr: Some(ToolOutputWriterEntry {
                producer: stderr_producer,
                completion_only: false,
            }),
        };
        let controllers = endpoints.controllers();

        for controller in controllers
            .0
            .iter()
            .chain(controllers.1.iter())
            .chain(controllers.2.iter())
        {
            controller
                .restore_terminal(
                    ByteStreamCloseCause::Failed(failure.clone()),
                    matches!(failure, ByteStreamFailure::ResourceExhausted),
                    golem_common::model::OplogIndex::from_u64(9),
                    false,
                )
                .unwrap();
        }
        super::publish_no_body_terminals(
            controllers.0.as_ref(),
            controllers.1.as_ref(),
            controllers.2.as_ref(),
        );
        drop(endpoints);

        assert_eq!(
            controllers.0.as_ref().unwrap().metadata().mode,
            ToolAttachmentModeMetadata::TerminalOnly
        );
        assert_eq!(
            controllers.1.as_ref().unwrap().metadata().mode,
            ToolAttachmentModeMetadata::TerminalOnly
        );
        assert_eq!(
            controllers.2.as_ref().unwrap().metadata().mode,
            ToolAttachmentModeMetadata::TerminalOnly
        );
        for observer in [&stdin_observer, &stdout_observer, &stderr_observer] {
            let Some(ByteStreamCloseCause::Failed(actual)) = observer.terminal() else {
                panic!("skipped replay endpoint did not retain its recorded failure");
            };
            assert!(matches!(
                (&failure, actual),
                (
                    ByteStreamFailure::ResourceExhausted,
                    ByteStreamFailure::ResourceExhausted
                ) | (ByteStreamFailure::Cancelled, ByteStreamFailure::Cancelled)
            ));
        }

        drop(stdin_producer);
        drop(stdout_consumer);
        drop(stderr_consumer);
    }
}

fn registered_tool() -> (RegisteredTool, ComponentId) {
    let component_id = ComponentId::new();
    let definition = Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "search".to_string(),
                aliases: vec!["find".to_string()],
                doc: Doc {
                    summary: "Search documents".to_string(),
                    description: "Searches indexed documents".to_string(),
                    examples: Vec::new(),
                },
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: Some(CommandBody {
                    positionals: Positionals {
                        fixed: vec![Positional {
                            name: "query".to_string(),
                            doc: Doc::default(),
                            value_name: Some("QUERY".to_string()),
                            type_: SchemaType::string().with_metadata(MetadataEnvelope {
                                doc: Some("Text to search for".to_string()),
                                aliases: vec!["term".to_string()],
                                ..Default::default()
                            }),
                            default: None,
                            required: true,
                            accepts_stdio: false,
                        }],
                        tail: None,
                    },
                    options: Vec::new(),
                    flags: Vec::new(),
                    constraints: Vec::new(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    result: None,
                    errors: Vec::new(),
                    annotations: None,
                }),
            }],
        },
        schema: SchemaGraph::empty(),
    };

    (
        RegisteredTool {
            deployment_revision: DeploymentRevision::try_from(1_u64).unwrap(),
            release_id: None,
            definition,
            provision: ToolProvisionConfig::default(),
            component_bindings: Default::default(),
            source: ToolSource::Component {
                component_id,
                component_revision: ComponentRevision::try_from(7_u64).unwrap(),
                component_name: ComponentName("search-tools".to_string()),
            },
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Default::default(),
        },
        component_id,
    )
}

fn dynamic_middleware_deployment(expected: Tool) -> (ToolDeploymentState, ToolBindingOwner) {
    let revision = DeploymentRevision::INITIAL;
    let tool_name = ToolName::try_from("search").unwrap();
    let middleware_name = ToolMiddlewareName::try_from("project").unwrap();
    let mut presented = expected.clone();
    presented.commands.nodes[0].name = "effective-search".to_string();
    presented.commands.nodes[0].doc.summary = "Effective projected metadata".to_string();
    let registration = RegisteredToolMiddleware {
        deployment_revision: revision,
        release_id: None,
        definition: ToolMiddleware {
            name: middleware_name.to_string(),
            version: "1.0.0".to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            parameter_schema: SchemaGraph::empty(),
            scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
                presented,
                expected: Some(expected),
            })),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolMiddlewareSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("middleware:project".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("middleware@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
    };
    let installation = ToolMiddlewareInstallation {
        name: middleware_name.clone(),
        version: Some("1.0.0".to_string()),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        account: Some(AccountEmail::new("middleware@example.com")),
        secret_keys_readable: None,
        secret_keys_revealable: None,
        filesystem_access: Default::default(),
    };
    let agent = AgentTypeName("Agent".to_string());
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent.clone(),
    };
    (
        ToolDeploymentState {
            deployment_revision: revision,
            registered_tools: BTreeMap::new(),
            tool_bindings: BTreeMap::new(),
            mcp_imports: Vec::new(),
            tool_middleware_configuration:
                golem_common::model::tool_middleware::ToolMiddlewareConfiguration {
                    universal: Vec::new(),
                    compatibility_mode: Default::default(),
                    environment_bindings: BTreeMap::from([(
                        tool_name,
                        ToolBindingInput {
                            middleware: Some(vec![installation]),
                            ..Default::default()
                        },
                    )]),
                    agent_bindings: BTreeMap::new(),
                },
            registered_tool_middlewares: BTreeMap::from([(middleware_name, registration)]),
            tool_middleware_chains: BTreeMap::new(),
        },
        owner,
    )
}

fn discovery_snapshot(tools: Vec<DiscoveredTool>) -> SerializableToolDiscoverySnapshot {
    SerializableToolDiscoverySnapshot {
        deployment_revision: Some(DeploymentRevision::INITIAL.get()),
        dynamic_tools: vec![SerializableMcpImportDiscovery {
            import_index: 0,
            tools: SerializableDiscoveredTools(tools),
            exclusions: Vec::new(),
        }],
    }
}

#[test]
fn dynamic_discovery_presents_effective_metadata_without_changing_lookup_name() {
    use golem_common::model::mcp_import::mcp_import_bridge_source;
    use golem_mcp_import::tool::{Limits, ProjectedTool};

    let projected = ProjectedTool::new(
        &serde_json::json!({
            "name": "search",
            "description": "Genuine upstream metadata",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "string" }
        }),
        "search",
        Limits::default(),
    )
    .unwrap();
    let discovered = DiscoveredTool::new(projected.definition.clone(), mcp_import_bridge_source());
    let (deployment, owner) = dynamic_middleware_deployment(projected.definition);
    let selected = ToolName::try_from("search").unwrap();

    for tools in [
        merge_discovered_tools(
            Some(&deployment),
            &owner,
            None,
            discovery_snapshot(vec![discovered.clone()]),
        )
        .unwrap(),
        merge_discovered_tools(
            Some(&deployment),
            &owner,
            Some(&selected),
            discovery_snapshot(vec![discovered]),
        )
        .unwrap(),
    ] {
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].lookup_name, "search");
        assert_eq!(tools[0].definition.name(), Some("effective-search"));
        assert_eq!(
            tools[0].definition.commands.nodes[0].doc.summary,
            "Effective projected metadata"
        );
    }
}

#[test]
fn dynamic_discovery_rejects_first_incompatible_collision_without_selecting_later_tool() {
    use golem_common::model::mcp_import::mcp_import_bridge_source;
    use golem_mcp_import::tool::{Limits, ProjectedTool};

    let compatible = ProjectedTool::new(
        &serde_json::json!({
            "name": "search",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        "search",
        Limits::default(),
    )
    .unwrap();
    let changed = ProjectedTool::new(
        &serde_json::json!({
            "name": "search",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "integer" } },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        "search",
        Limits::default(),
    )
    .unwrap();
    let (deployment, owner) = dynamic_middleware_deployment(compatible.definition.clone());
    let selected = ToolName::try_from("search").unwrap();
    let error = merge_discovered_tools(
        Some(&deployment),
        &owner,
        Some(&selected),
        discovery_snapshot(vec![
            DiscoveredTool::new(changed.definition, mcp_import_bridge_source()),
            DiscoveredTool::new(compatible.definition, mcp_import_bridge_source()),
        ]),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("middleware for dynamic tool 'search' is incompatible")
    );
}

#[test]
fn registered_component_tool_converts_to_discovery_wit_record() {
    let (registered, component_id) = registered_tool();
    let expected_definition = registered.definition.clone();

    let discovered = DiscoveredTool::from(registered);
    assert_eq!(discovered.lookup_name, "search");
    assert_eq!(discovered.definition, expected_definition);
    assert_eq!(discovered.implemented_by, component_id);

    let wit = WitRegisteredTool::try_from(&discovered).unwrap();
    assert_eq!(
        Tool::try_from(&wit.definition).unwrap(),
        expected_definition
    );
    assert_eq!(ComponentId::from(wit.implemented_by), component_id);
    assert_eq!(wit.lookup_name, "search");
}

#[test]
fn tool_authorization_owner_is_entirely_caller_derived() {
    let caller_account = AccountEmail::new("consumer@example.com");
    let publisher_account = AccountEmail::new("publisher@example.com");
    let application = ApplicationName::try_from("consumer-application").unwrap();
    let environment = EnvironmentName::try_from("consumer-environment").unwrap();
    let component = ComponentName("consumer:agent".to_string());
    let tool_name = ToolName::try_from("search").unwrap();

    let owner = caller_tool_owner(
        &caller_account,
        &application,
        &environment,
        &component,
        &tool_name,
    );

    assert_eq!(
        owner,
        ToolOwnerPattern::Tool {
            account: caller_account.clone(),
            application,
            environment,
            component,
            tool: tool_name.to_string(),
        }
    );
    let ToolOwnerPattern::Tool { account, .. } = owner else {
        panic!("authorization owner must identify one caller-owned tool")
    };
    assert_eq!(account, caller_account);
    assert_ne!(account, publisher_account);
}

#[test]
fn builtin_git_structural_permissions_authorize_typed_inputs() {
    use crate::durable_host::authorization::targets::tool_target;
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{
        EffectiveSurface, PermissionPattern, ToolResourcePattern, parse_permission,
    };
    use golem_common::schema::tool::argv::{self, ParsedToolArguments};

    // Extracted in-process from git-tool-v0.0.13/git-tool.wasm, pinned in
    // builtin-artifacts.lock.json (SHA-256 4859d21e463bfbde91db409f546f65ecb1945616c0495353a30b05d0e3b0d1e5).
    let tool: Tool = serde_json::from_str(include_str!(
        "../../../../test-data/git-tool-definition.json"
    ))
    .unwrap();
    let holder = RecipientPattern::parse("acme/app/env/component/agent").unwrap();
    let grant = |resource: &str| {
        let mut grant =
            parse_permission("tool(acme/app/env/component/git) @ * : invoke : *").unwrap();
        let PermissionPattern::Tool(pattern) = &mut grant else {
            unreachable!()
        };
        pattern.resource =
            ToolResourcePattern::parse_with_definition(resource, Some(&tool)).unwrap();
        grant
    };
    let target = |args: Vec<String>| {
        let ParsedToolArguments::Invoke {
            command_path,
            input,
        } = argv::parse(&tool, &args).unwrap()
        else {
            panic!("expected invocation")
        };
        let resolved = resolve_tool_command(&tool, &command_path, &input).unwrap();
        tool_target(
            ToolOwnerPattern::parse("acme/app/env/component/git").unwrap(),
            &command_path.iter().map(String::as_str).collect::<Vec<_>>(),
            resolved.args,
            Some(&tool),
        )
        .unwrap()
    };
    for (cwd, allowed) in [
        ("/workspace/project", true),
        ("/workspace/project/src", true),
        ("/private/project", false),
        ("/workspace", false),
    ] {
        let request = target(vec![
            "status".into(),
            "--short".into(),
            format!("--cwd={cwd}"),
        ]);
        assert!(
            EffectiveSurface::from_grants(&[grant("status")], &[], &[], &[], &holder)
                .unwrap()
                .authorize(&request)
                .unwrap()
        );
        assert_eq!(
            EffectiveSurface::from_grants(
                &[grant("status --cwd=/workspace/**")],
                &[],
                &[],
                &[],
                &holder
            )
            .unwrap()
            .authorize(&request)
            .unwrap(),
            allowed
        );
    }
    for upper in [false, true] {
        let denial = [grant("status -sz")];
        let surface = EffectiveSurface::from_grants(
            &[grant("status")],
            if upper { &[] } else { &denial },
            &[],
            if upper { &denial } else { &[] },
            &holder,
        )
        .unwrap();
        for flags in [vec!["-sz"], vec!["-zs"], vec!["-z", "--short"]] {
            let mut args = vec!["status".into(), "--cwd=/workspace/project".into()];
            args.extend(flags.into_iter().map(str::to_string));
            assert!(!surface.authorize(&target(args)).unwrap());
        }
        assert!(
            surface
                .authorize(&target(vec!["status".into(), "-s".into()]))
                .unwrap()
        );
    }
    let surface = EffectiveSurface::from_grants(
        &[grant("status")],
        &[grant("status -C=/private/**")],
        &[],
        &[],
        &holder,
    )
    .unwrap();
    for forbidden_index in 0..3 {
        let mut args = vec!["status".into()];
        for index in 0..3 {
            args.push(format!(
                "--working-directory={}",
                if index == forbidden_index {
                    "/private/repo"
                } else {
                    "/workspace/repo"
                }
            ));
        }
        assert!(!surface.authorize(&target(args)).unwrap());
    }
    let request = target(vec!["checkout".into(), "--".into(), "private/file".into()]);
    assert!(
        EffectiveSurface::from_grants(&[grant("checkout -- private/**")], &[], &[], &[], &holder)
            .unwrap()
            .authorize(&request)
            .unwrap()
    );
    for upper in [false, true] {
        let denials = [grant("checkout -- private/**")];
        let surface = EffectiveSurface::from_grants(
            &[grant("checkout")],
            if upper { &[] } else { &denials },
            &[],
            if upper { &denials } else { &[] },
            &holder,
        )
        .unwrap();
        assert!(!surface.authorize(&request).unwrap());
    }

    let mut aliased_tool = tool.clone();
    aliased_tool
        .commands
        .nodes
        .iter_mut()
        .find(|node| node.name == "status")
        .unwrap()
        .aliases
        .push("st".into());
    let ParsedToolArguments::Invoke { input, .. } =
        argv::parse(&aliased_tool, &["status".into(), "--short".into()]).unwrap()
    else {
        panic!("expected invocation")
    };
    let descriptor = super::describe_tool_command(
        &aliased_tool,
        &input,
        0,
        vec!["st".into()],
        false,
        false,
        false,
        EntityCallMode::Synchronous,
    )
    .unwrap();
    let alias_target = tool_target(
        ToolOwnerPattern::parse("acme/app/env/component/git").unwrap(),
        &["st"],
        descriptor.args,
        Some(&aliased_tool),
    )
    .unwrap();
    assert_eq!(
        alias_target,
        target(vec!["status".into(), "--short".into()])
    );
    assert_eq!(descriptor.command_path, vec!["st".to_string()]);
}

#[test]
fn tool_boolean_denials_match_effective_values_including_defaults() {
    use crate::durable_host::authorization::targets::tool_target;
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{EffectiveSurface, parse_permission};
    use golem_common::schema::tool::{BoolFlagShape, FlagShape, FlagSpec};

    for default in [false, true] {
        for value in [false, true] {
            let (mut registered, _) = registered_tool();
            registered.definition.commands.nodes[0]
                .body
                .as_mut()
                .unwrap()
                .flags
                .push(FlagSpec {
                    long: "force".into(),
                    short: Some('f'),
                    aliases: vec![],
                    doc: Doc::default(),
                    shape: FlagShape::BoolFlag(BoolFlagShape {
                        default,
                        negatable: true,
                    }),
                    env_var: None,
                });
            let input = TypedSchemaValue::new(
                registered
                    .definition
                    .canonical_input_record_schema(0)
                    .unwrap(),
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("needle".into()),
                        SchemaValue::Bool(value),
                    ],
                },
            );
            let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
            let target = tool_target(
                ToolOwnerPattern::parse("acme/app/env/component/tool").unwrap(),
                &[],
                resolved.args,
                Some(&registered.definition),
            )
            .unwrap();
            let grant = |resource| {
                parse_permission(&format!(
                    "tool(acme/app/env/component/tool) @ * : invoke : {resource}"
                ))
                .unwrap()
            };
            let holder = RecipientPattern::parse("acme/app/env/component/agent").unwrap();
            for (denial, expected) in [(". --force", !value), (". --no-force", value)] {
                let surface = EffectiveSurface::from_grants(
                    &[grant(".")],
                    &[grant(denial)],
                    &[],
                    &[],
                    &holder,
                )
                .unwrap();
                assert_eq!(
                    surface.authorize(&target).unwrap(),
                    expected,
                    "default={default}, value={value}, denial={denial}"
                );
            }
        }
    }
}

#[test]
fn count_flag_respects_declared_optional_maximum() {
    use golem_common::schema::tool::{FlagShape, FlagSpec};

    for max in [None, Some(200)] {
        let (mut registered, _) = registered_tool();
        registered.definition.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .flags
            .push(FlagSpec {
                long: "verbose".into(),
                short: Some('v'),
                aliases: vec![],
                doc: Doc::default(),
                shape: FlagShape::CountFlag(max),
                env_var: None,
            });
        let input = TypedSchemaValue::new(
            registered
                .definition
                .canonical_input_record_schema(0)
                .unwrap(),
            SchemaValue::Record {
                fields: vec![SchemaValue::String("needle".into()), SchemaValue::U32(129)],
            },
        );
        let result = resolve_tool_command(&registered.definition, &[], &input);
        assert!(
            result.is_ok(),
            "count 129 is within declared maximum {max:?}: {:?}",
            result.err()
        );
    }
}

#[test]
fn command_resolution_validates_path_schema_value_and_constraints() {
    let (registered, _) = registered_tool();
    let graph = registered
        .definition
        .canonical_input_record_schema(0)
        .unwrap();
    let input = TypedSchemaValue::new(
        graph,
        SchemaValue::Record {
            fields: vec![SchemaValue::String("needle".to_string())],
        },
    );

    let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
    assert_eq!(
        resolved.args,
        vec![ToolArgPattern::Positional(ToolInvocationValue::Literal(
            "needle".to_string()
        ))]
    );
    assert!(matches!(
        resolve_tool_command(&registered.definition, &["missing".to_string()], &input),
        Err(SerializableToolError::InvalidCommandPath(_))
    ));

    let wrong_schema = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::bool()),
        SchemaValue::Bool(true),
    );
    assert!(matches!(
        resolve_tool_command(&registered.definition, &[], &wrong_schema),
        Err(SerializableToolError::InvalidInput(_))
    ));

    let mut constrained = registered.definition.clone();
    constrained.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .constraints = vec![Constraint::RequiresAll(vec![Ref::Present(
        "not-supplied".to_string(),
    )])];
    assert!(matches!(
        resolve_tool_command(&constrained, &[], &input),
        Err(SerializableToolError::ConstraintViolation(_))
    ));
}

#[test]
fn command_resolution_omits_optional_values_and_renders_supplied_values() {
    let (mut registered, _) = registered_tool();
    let body = registered.definition.commands.nodes[0]
        .body
        .as_mut()
        .unwrap();
    body.positionals.fixed[0].required = false;
    body.options.push(OptionSpec {
        long: "mode".to_string(),
        short: None,
        aliases: Vec::new(),
        doc: Doc::default(),
        value_name: None,
        shape: OptionShape::Scalar(SchemaType::string()),
        default: None,
        required: false,
        env_var: None,
    });
    let graph = registered
        .definition
        .canonical_input_record_schema(0)
        .unwrap();
    for (fields, expected) in [
        (
            vec![
                SchemaValue::Option { inner: None },
                SchemaValue::Option { inner: None },
            ],
            Vec::<ToolArgPattern<ToolInvocationValue>>::new(),
        ),
        (
            vec![
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("needle".to_string()))),
                },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("fast".to_string()))),
                },
            ],
            vec![
                ToolArgPattern::Positional(ToolInvocationValue::Literal("needle".to_string())),
                ToolArgPattern::LongFlag {
                    name: ToolIdentifier("mode".to_string()),
                    value: Some(ToolInvocationValue::Literal("fast".to_string())),
                },
            ],
        ),
    ] {
        let input = TypedSchemaValue::new(graph.clone(), SchemaValue::Record { fields });
        let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
        assert_eq!(resolved.args, expected);
    }
}

#[test]
fn stream_arguments_require_opaque_permission_without_losing_literal_constraints() {
    use golem_common::model::card::{ResourcePattern, ToolInvocation, ToolResourcePattern};

    let (mut registered, _) = registered_tool();
    let body = registered.definition.commands.nodes[0]
        .body
        .as_mut()
        .unwrap();
    let mut stream_argument = body.positionals.fixed[0].clone();
    stream_argument.name = "input".to_string();
    stream_argument.type_ = SchemaType::stream(Some(SchemaType::u32()));
    body.positionals.fixed.push(stream_argument);
    let input = TypedSchemaValue::new(
        registered
            .definition
            .canonical_input_record_schema(0)
            .unwrap(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("needle".to_string()),
                SchemaValue::Stream(SchemaValueStream::from_host_endpoint(())),
            ],
        },
    );
    let resolved = resolve_tool_command(&registered.definition, &[], &input).unwrap();
    assert_eq!(
        resolved.args,
        vec![
            ToolArgPattern::Positional(ToolInvocationValue::Literal("needle".to_string())),
            ToolArgPattern::Positional(ToolInvocationValue::Opaque),
        ]
    );
    let target = ToolResourcePattern::Request(ToolInvocation {
        command_path: vec![ToolIdentifier("consume".to_string())],
        flags: vec![],
        positionals: vec![
            ToolInvocationValue::Literal("needle".into()),
            ToolInvocationValue::Opaque,
        ],
    });
    assert!(
        ToolResourcePattern::parse_resource("consume needle **")
            .unwrap()
            .subsumes(&target)
    );
    assert!(
        !ToolResourcePattern::parse_resource("consume other **")
            .unwrap()
            .subsumes(&target)
    );
    assert!(
        !ToolResourcePattern::parse_resource("consume needle literal")
            .unwrap()
            .subsumes(&target)
    );
    assert!(ToolResourcePattern::AnyInvocation.subsumes(&target));
}

#[test]
fn present_optional_option_value_renders_its_inner_type() {
    let (registered, _) = registered_tool();
    let option = OptionSpec {
        long: "limit".to_string(),
        short: None,
        aliases: vec![],
        doc: Doc::default(),
        value_name: None,
        default: None,
        required: false,
        env_var: None,
        shape: OptionShape::Scalar(SchemaType::option(SchemaType::s64())),
    };
    assert!(
        option_args(
            &registered.definition,
            &option,
            &SchemaValue::Option { inner: None }
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        option_args(
            &registered.definition,
            &option,
            &SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::S64(3)))
            }
        )
        .unwrap(),
        vec![ToolArgPattern::LongFlag {
            name: ToolIdentifier("limit".to_string()),
            value: Some(ToolInvocationValue::Literal("3".to_string()))
        }]
    );
}

#[test]
fn present_optional_option_with_referenced_option_type_preserves_inner_option() {
    let (mut registered, _) = registered_tool();
    let optional_limit = TypeId::new("optional-limit");
    registered.definition.schema.defs.push(SchemaTypeDef {
        id: optional_limit.clone(),
        name: Some("OptionalLimit".to_string()),
        body: SchemaType::option(SchemaType::s64()),
    });
    let option = OptionSpec {
        long: "limit".to_string(),
        short: None,
        aliases: vec![],
        doc: Doc::default(),
        value_name: None,
        default: None,
        required: false,
        env_var: None,
        shape: OptionShape::Scalar(SchemaType::ref_to(optional_limit)),
    };
    let value = SchemaValue::Option {
        inner: Some(Box::new(SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::S64(3))),
        })),
    };

    assert_eq!(
        option_args(&registered.definition, &option, &value).unwrap(),
        vec![ToolArgPattern::LongFlag {
            name: ToolIdentifier("limit".to_string()),
            value: Some(ToolInvocationValue::Literal("some(3)".to_string()))
        }]
    );
}

#[test]
fn stream_option_values_preserve_repetition_boundaries() {
    use golem_common::schema::tool::RepeatableListShape;

    let (registered, _) = registered_tool();
    let mut option = OptionSpec {
        long: "input".to_string(),
        short: None,
        aliases: vec![],
        doc: Doc::default(),
        value_name: None,
        default: None,
        required: true,
        env_var: None,
        shape: OptionShape::RepeatableList(RepeatableListShape {
            repetition: Repetition::Repeated,
            item_type: SchemaType::option(SchemaType::stream(Some(SchemaType::u32()))),
        }),
    };
    let value = SchemaValue::List {
        elements: vec![
            SchemaValue::Option { inner: None },
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Stream(
                    SchemaValueStream::from_host_endpoint(()),
                ))),
            },
        ],
    };
    let repeated = option_args(&registered.definition, &option, &value).unwrap();
    assert_eq!(repeated.len(), 2);
    assert!(matches!(
        &repeated[0],
        ToolArgPattern::LongFlag {
            value: Some(ToolInvocationValue::Literal(_)),
            ..
        }
    ));
    let opaque = ToolArgPattern::LongFlag {
        name: ToolIdentifier("input".to_string()),
        value: Some(ToolInvocationValue::Opaque),
    };
    assert_eq!(repeated[1], opaque);
    for repetition in [Repetition::Delimited(','), Repetition::Either(':')] {
        let OptionShape::RepeatableList(shape) = &mut option.shape else {
            unreachable!()
        };
        shape.repetition = repetition;
        assert_eq!(
            option_args(&registered.definition, &option, &value).unwrap(),
            repeated
        );
    }
}

#[test]
fn declared_tool_error_validation_uses_name_before_payload() {
    let string = SchemaGraph::anonymous(SchemaType::string());
    let contract = ToolOutputContract {
        result: None,
        errors: vec![
            NamedToolErrorSchema {
                name: "first".to_string(),
                payload: string.clone(),
            },
            NamedToolErrorSchema {
                name: "second".to_string(),
                payload: string.clone(),
            },
        ],
    };
    let error = SerializableToolRpcError::RemoteToolError(Box::new(
        SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
            name: "second".to_string(),
            payload: TypedSchemaValue::new(string, SchemaValue::String("details".to_string())),
        })),
    ));

    assert_eq!(
        validate_declared_tool_error(error.clone(), &contract, false),
        error
    );
}

#[test]
fn universal_middleware_may_return_well_formed_undeclared_custom_error() {
    let payload_schema = SchemaGraph::anonymous(SchemaType::string());
    let contract = ToolOutputContract {
        result: None,
        errors: Vec::new(),
    };
    let error = SerializableToolRpcError::RemoteToolError(Box::new(
        SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
            name: "path-policy-denied".to_string(),
            payload: TypedSchemaValue::new(
                payload_schema,
                SchemaValue::String("outside allowed roots".to_string()),
            ),
        })),
    ));

    assert_eq!(
        validate_declared_tool_error(error.clone(), &contract, true),
        error
    );
    assert!(matches!(
        validate_declared_tool_error(error, &contract, false),
        SerializableToolRpcError::RemoteToolError(error)
            if matches!(*error, SerializableToolError::InvalidResult(_))
    ));
}

#[test]
fn universal_middleware_rejects_undeclared_custom_error_with_malformed_schema() {
    let payload_schema = SchemaGraph::anonymous(SchemaType::record_from_fields([
        ("reason", SchemaType::string()),
        ("reason", SchemaType::string()),
    ]));
    let contract = ToolOutputContract {
        result: None,
        errors: Vec::new(),
    };
    let error = SerializableToolRpcError::RemoteToolError(Box::new(
        SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
            name: "path-policy-denied".to_string(),
            payload: TypedSchemaValue::new(
                payload_schema,
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("first".to_string()),
                        SchemaValue::String("second".to_string()),
                    ],
                },
            ),
        })),
    ));

    assert!(matches!(
        validate_declared_tool_error(error, &contract, true),
        SerializableToolRpcError::RemoteToolError(error)
            if matches!(*error, SerializableToolError::InvalidResult(_))
    ));
}

#[test]
fn declared_payloadless_tool_error_requires_unit_payload() {
    let unit = SchemaGraph::anonymous(SchemaType::tuple(Vec::new()));
    let contract = ToolOutputContract {
        result: None,
        errors: vec![NamedToolErrorSchema {
            name: "not-found".to_string(),
            payload: unit.clone(),
        }],
    };
    let error = SerializableToolRpcError::RemoteToolError(Box::new(
        SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
            name: "not-found".to_string(),
            payload: TypedSchemaValue::new(
                unit,
                SchemaValue::Tuple {
                    elements: Vec::new(),
                },
            ),
        })),
    ));

    assert_eq!(
        validate_declared_tool_error(error.clone(), &contract, false),
        error
    );
}

#[test]
fn ordinary_payloadless_tool_result_remains_valid() {
    let contract = ToolOutputContract {
        result: None,
        errors: Vec::new(),
    };

    assert_eq!(validate_declared_tool_result(None, &contract), Ok(None));
}

#[test]
fn native_output_validation_accepts_a_typed_declared_result() {
    let schema = SchemaGraph::anonymous(SchemaType::u64());
    let value = TypedSchemaValue::new(schema.clone(), SchemaValue::U64(42));
    let result = SerializableToolStructuredResult {
        result: Some(SerializableToolResultValue::from_typed(&value).unwrap()),
    };
    let contract = ToolOutputContract {
        result: Some(schema),
        errors: Vec::new(),
    };

    assert_eq!(
        validate_native_tool_output(Ok(result.clone()), &contract),
        Ok(result)
    );
}

#[test]
fn native_output_validation_rejects_undeclared_error_name_and_wrong_result_schema() {
    let string = SchemaGraph::anonymous(SchemaType::string());
    let contract = ToolOutputContract {
        result: Some(string.clone()),
        errors: vec![NamedToolErrorSchema {
            name: "declared".to_string(),
            payload: string,
        }],
    };
    let undeclared = SerializableToolRpcError::RemoteToolError(Box::new(
        SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
            name: "other".to_string(),
            payload: TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::string()),
                SchemaValue::String("details".to_string()),
            ),
        })),
    ));
    let wrong_result = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::bool()),
        SchemaValue::Bool(true),
    );

    for result in [
        validate_native_tool_output(Err(undeclared), &contract),
        validate_native_tool_output(
            Ok(SerializableToolStructuredResult {
                result: Some(SerializableToolResultValue::from_typed(&wrong_result).unwrap()),
            }),
            &contract,
        ),
    ] {
        assert!(matches!(
            result,
            Err(SerializableToolRpcError::RemoteToolError(error))
                if matches!(*error, SerializableToolError::InvalidResult(_))
        ));
    }
}

#[test]
fn stream_attachment_validation_uses_actual_attachments_and_call_mode() {
    let mut command = ResolvedToolCommand {
        command_index: 0,
        args: Vec::new(),
        stdin_required: None,
        stdout_required: None,
        stderr_required: None,
    };
    assert!(matches!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            true,
            false,
            false,
            EntityCallMode::Synchronous,
        ),
        Err(SerializableToolRpcError::ProtocolError(_))
    ));
    assert!(matches!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            true,
            false,
            EntityCallMode::Synchronous,
        ),
        Err(SerializableToolRpcError::ProtocolError(_))
    ));
    assert!(matches!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            false,
            true,
            EntityCallMode::Synchronous,
        ),
        Err(SerializableToolRpcError::ProtocolError(_))
    ));
    assert!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            false,
            false,
            EntityCallMode::FireAndForget,
        )
        .is_ok()
    );

    command.stdout_required = Some(false);
    command.stderr_required = Some(true);
    assert!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            false,
            true,
            EntityCallMode::Synchronous,
        )
        .is_ok()
    );
    assert!(matches!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            true,
            false,
            EntityCallMode::Synchronous,
        ),
        Err(SerializableToolRpcError::ProtocolError(message)) if message.contains("stderr")
    ));

    command.stdout_required = Some(true);
    command.stderr_required = Some(false);
    assert!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            true,
            false,
            EntityCallMode::Synchronous,
        )
        .is_ok()
    );
    assert!(matches!(
        validate_stream_attachments(
            &command,
            &["search".to_string()],
            false,
            false,
            true,
            EntityCallMode::Synchronous,
        ),
        Err(SerializableToolRpcError::ProtocolError(message)) if message.contains("stdout")
    ));
}

#[test]
fn tool_discovery_error_classification_preserves_integrity_semantics() {
    let retrieval = ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime("offline"));
    let agent_context_required = ToolDiscoveryError::AgentContextRequired;
    let inconsistent = ToolDiscoveryError::InconsistentSnapshot {
        details: "dangling binding".to_string(),
    };

    assert_eq!(
        classify_tool_discovery_error(&retrieval),
        HostFailureKind::Transient
    );
    assert_eq!(
        classify_tool_discovery_error(&agent_context_required),
        HostFailureKind::Permanent
    );
    assert_eq!(
        classify_tool_discovery_error(&inconsistent),
        HostFailureKind::Permanent
    );
}

#[test]
fn terminal_tool_discovery_errors_remain_permanent() {
    let error = terminal_tool_discovery_error("discovery failed".to_string());
    let classified = error.downcast_ref::<ClassifiedHostError>().unwrap();

    assert_eq!(classified.kind, HostFailureKind::Permanent);
    assert_eq!(classified.message, "discovery failed");
}

#[test]
fn discovered_tool_wit_encoding_error_has_tool_and_source_context() {
    let (registered, component_id) = registered_tool();
    let mut discovered = DiscoveredTool::from(registered);
    let duplicate_id = TypeId::new("duplicate");
    let duplicate_definition = SchemaTypeDef {
        id: duplicate_id,
        name: Some("Duplicate".to_string()),
        body: SchemaType::string(),
    };
    discovered.definition.schema.defs = vec![duplicate_definition.clone(), duplicate_definition];

    let error = match WitRegisteredTool::try_from(&discovered) {
        Ok(_) => panic!("duplicate schema definitions must fail WIT encoding"),
        Err(error) => error,
    };
    let message = format!("{error:#}");

    assert!(message.contains("discovered tool 'search'"));
    assert!(message.contains(&component_id.to_string()));
    assert!(message.contains("duplicate type id: duplicate"));
}
