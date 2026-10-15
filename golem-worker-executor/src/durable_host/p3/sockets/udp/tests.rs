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

use super::interruptible_receive;
use crate::durable_host::durability::{DurableCallTrapContext, mark_durable_call_trap_context};
use crate::model::TrapType;
use crate::workerctx::default::Context;
use golem_common::model::agent::AgentMode;
use golem_common::model::{OplogIndex, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::{pending, poll_fn, ready};
use std::task::Poll;
use test_r::{test, timeout};
use wasmtime_wasi::p3::sockets::SocketResult;

fn assert_interrupt(result: wasmtime::Result<SocketResult<Vec<u8>>>, expected: InterruptKind) {
    let error = result.unwrap_err();
    assert_eq!(
        error.root_cause().downcast_ref::<InterruptKind>(),
        Some(&expected)
    );
    // run_read_access's invoke_access driver abandons the Start with this marker.
    // Classification must still see the original typed cause, never a socket error value.
    let marked = mark_durable_call_trap_context(
        error.into(),
        DurableCallTrapContext {
            retry_from: OplogIndex::from_u64(12),
            in_atomic_region: false,
        },
    );
    assert!(matches!(TrapType::from_error::<Context>(
        &marked, OplogIndex::INITIAL, false, false, AgentMode::Durable,
    ), TrapType::Interrupt(kind) if kind == expected));
}

#[test]
#[timeout("10s")]
async fn prepublished_signal_interrupts_receive() {
    let kind = InterruptKind::Suspend(Timestamp::now_utc());
    assert_interrupt(interruptible_receive(pending(), ready(kind)).await, kind);
}

#[test]
#[timeout("10s")]
async fn signal_interrupts_pending_receive() {
    let kind = InterruptKind::Suspend(Timestamp::now_utc());
    let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
    let (signal_tx, signal_rx) = tokio::sync::oneshot::channel();
    let mut pending_tx = Some(pending_tx);
    let native = poll_fn(|_| {
        if let Some(tx) = pending_tx.take() {
            let _ = tx.send(());
        }
        Poll::Pending
    });
    let receive = interruptible_receive(native, async { signal_rx.await.unwrap() });
    let trigger = async {
        pending_rx.await.unwrap();
        signal_tx.send(kind).unwrap();
    };
    let (result, ()) = tokio::join!(receive, trigger);
    assert_interrupt(result, kind);
}

#[test]
#[timeout("10s")]
async fn receive_completion_before_signal_keeps_datagram() {
    let (signal_tx, signal_rx) = tokio::sync::oneshot::channel();
    let result =
        interruptible_receive(ready(Ok(vec![b'x'])), async { signal_rx.await.unwrap() }).await;
    assert_eq!(result.unwrap().unwrap(), vec![b'x']);
    assert!(
        signal_tx
            .send(InterruptKind::Suspend(Timestamp::now_utc()))
            .is_err()
    );
}
