// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::*;
use golem_common::model::oplog::attachment::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError, ToolAttachmentId,
    ToolAttachmentSlot, ToolAttachmentTerminal,
};
use golem_common::model::oplog::payload::{
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemToolTools, HostResponseRandomBytes,
    HostResponseToolAttachmentClosed, HostResponseToolAttachmentCreated,
    HostResponseToolAttachmentTerminalResult, HostResponseToolAttachmentWrite,
    tool_attachment::{
        ToolAttachmentCreated, ToolAttachmentTerminalOutcome, ToolAttachmentWriteOutcome,
    },
};
use test_r::test;

fn assert_response_roundtrip<Resp>(response: Resp)
where
    Resp: Into<HostResponse>
        + TryFrom<HostResponse, Error = String>
        + Clone
        + PartialEq
        + std::fmt::Debug,
{
    let expected: HostResponse = response.clone().into();
    let (returned, bytes) = *encode_response(Box::new(response.clone())).unwrap();
    assert_eq!(*returned, response);
    assert_eq!(
        bytes,
        golem_common::serialization::serialize(&expected).unwrap()
    );
    assert_eq!(
        golem_common::serialization::deserialize::<HostResponse>(&bytes).unwrap(),
        expected
    );
}

#[test]
fn response_encoding_preserves_payload() {
    assert_response_roundtrip(HostResponseRandomBytes {
        bytes: vec![1, 2, 3, 4],
    });
    assert!(response_has_bounded_encoding_stack::<HostResponseRandomBytes>());
    assert!(!response_has_bounded_encoding_stack::<
        HostResponseGolemRpcInvokeAndAwait,
    >());
    assert!(!response_has_bounded_encoding_stack::<
        HostResponseGolemToolTools,
    >());
}

#[test]
async fn response_encoding_on_wasmtime_fiber() -> anyhow::Result<()> {
    let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
    let engine = wasmtime::Engine::new(&config)?;
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
                (import "host" "encode" (func $encode))
                (func $nested (param $depth i32) (result i32)
                    local.get $depth
                    if (result i32)
                        local.get $depth
                        i32.const 1
                        i32.sub
                        call $nested
                        i32.const 1
                        i32.add
                    else
                        call $encode
                        i32.const 0
                    end)
                (func (export "run") (result i32)
                    i32.const 512
                    call $nested))"#,
    )?;
    let mut linker = wasmtime::Linker::new(&engine);
    linker.func_wrap("host", "encode", || -> wasmtime::Result<()> {
        for slot in [
            ToolAttachmentSlot::Stdin,
            ToolAttachmentSlot::Stdout,
            ToolAttachmentSlot::Stderr,
            ToolAttachmentSlot::Output,
        ] {
            assert_response_roundtrip(HostResponseToolAttachmentCreated {
                created: ToolAttachmentCreated {
                    attachment: ToolAttachmentId {
                        creation_start: OplogIndex::from_u64(37),
                        slot,
                    },
                    capacity_bytes: 4099,
                },
            });
        }
        for outcome in [
            ToolAttachmentWriteOutcome::Accepted,
            ToolAttachmentWriteOutcome::Empty,
            ToolAttachmentWriteOutcome::Discarded,
            ToolAttachmentWriteOutcome::HostResourceExhausted,
            ToolAttachmentWriteOutcome::Rejected(StreamWriteError::ConcurrentOperation),
        ] {
            assert_response_roundtrip(HostResponseToolAttachmentWrite { outcome });
        }
        for outcome in [
            ToolAttachmentTerminalOutcome::Unchanged,
            ToolAttachmentTerminalOutcome::Selected(ToolAttachmentTerminal::HostResourceExhausted),
            ToolAttachmentTerminalOutcome::Rejected(StreamWriteError::ConcurrentOperation),
        ] {
            assert_response_roundtrip(HostResponseToolAttachmentTerminalResult { outcome });
        }
        for length in [0, 4, 64 * 1024] {
            assert_response_roundtrip(HostResponseRandomBytes {
                bytes: vec![42; length],
            });
            for cause in [
                ByteStreamCloseCause::Finished,
                ByteStreamCloseCause::ConsumerCancelled,
                ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled),
                ByteStreamCloseCause::Failed(ByteStreamFailure::Abandoned),
                ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
                ByteStreamCloseCause::Failed(ByteStreamFailure::Failed("é".repeat(length))),
            ] {
                assert_response_roundtrip(HostResponseToolAttachmentClosed {
                    cause: cause.clone(),
                });
                assert_response_roundtrip(HostResponseToolAttachmentWrite {
                    outcome: ToolAttachmentWriteOutcome::Rejected(StreamWriteError::Closed(
                        cause.clone(),
                    )),
                });
                assert_response_roundtrip(HostResponseToolAttachmentTerminalResult {
                    outcome: ToolAttachmentTerminalOutcome::Selected(
                        ToolAttachmentTerminal::Application(cause.clone()),
                    ),
                });
                assert_response_roundtrip(HostResponseToolAttachmentTerminalResult {
                    outcome: ToolAttachmentTerminalOutcome::Rejected(StreamWriteError::Closed(
                        cause,
                    )),
                });
            }
        }
        Ok(())
    })?;
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_fuel(u64::MAX)?;
    store.set_epoch_deadline(u64::MAX);
    let instance = linker.instantiate_async(&mut store, &module).await?;
    let run = instance.get_typed_func::<(), i32>(&mut store, "run")?;
    assert_eq!(run.call_async(&mut store, ()).await?, 512);
    Ok(())
}
