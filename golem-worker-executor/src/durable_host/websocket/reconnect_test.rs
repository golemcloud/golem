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

//! Test-only observation of the websocket reconnect path taken by the
//! `receive` / `receive-with-timeout` / `send` / `close` helpers when they find
//! a `Replay` connection entry. The executor-side reconnect is coordinated
//! per resource handle, and tests need to drive it deterministically (pause the
//! handshake, observe which call wins the coordination) to reproduce
//! reconnect races that only manifest when several concurrent accessor calls
//! see the same replayed handle.

use golem_common::model::{IdempotencyKey, OplogIndex};
use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Poll;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketReconnectPathForTest {
    Accessor,
    Direct,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketReconnectWaitForTest {
    /// Waiting for the per-resource reconnect gate (another accessor call is
    /// already reconnecting this handle).
    Gate,
    /// Waiting for a connection-pool permit.
    Pool,
    /// Waiting for the websocket handshake itself.
    Handshake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketReconnectWaitStateForTest {
    /// The observed wait is still parked.
    Pending,
    /// The observed wait returned while the observer was alive.
    Returned,
    /// The observer was dropped while the wait was still parked
    /// (the call was interrupted/abandoned while waiting).
    Dropped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketReconnectOutcomeForTest {
    Live,
    Terminal,
}

#[derive(Debug)]
pub enum WebSocketReconnectEventKindForTest {
    /// A call holding the reconnect gate decided to perform the live
    /// reconnect for the replayed handle.
    Decided,
    /// A reconnect-path wait parked on its first poll.
    WaitPending { wait: WebSocketReconnectWaitForTest },
    /// The reconnect completed and published its outcome into the table.
    Published {
        outcome: WebSocketReconnectOutcomeForTest,
    },
}

#[derive(Debug)]
pub struct WebSocketReconnectEventForTest {
    pub invocation_key: Option<IdempotencyKey>,
    pub start: OplogIndex,
    pub path: WebSocketReconnectPathForTest,
    pub kind: WebSocketReconnectEventKindForTest,
    pub(crate) wait_state: Option<Arc<AtomicU8>>,
}

impl WebSocketReconnectEventForTest {
    pub fn wait_state(&self) -> Option<WebSocketReconnectWaitStateForTest> {
        Some(match self.wait_state.as_ref()?.load(Ordering::Acquire) {
            0 => WebSocketReconnectWaitStateForTest::Pending,
            1 => WebSocketReconnectWaitStateForTest::Returned,
            2 => WebSocketReconnectWaitStateForTest::Dropped,
            _ => unreachable!(),
        })
    }
}

/// Wraps a reconnect wait future so the test sees when it parks on its first
/// poll, and whether it returned or was abandoned while parked.
pub(crate) struct WebSocketReconnectWaitObserverForTest {
    pub(super) notify: mpsc::UnboundedSender<WebSocketReconnectEventForTest>,
    pub(super) event: Option<WebSocketReconnectEventForTest>,
    pub(super) state: Arc<AtomicU8>,
}

impl WebSocketReconnectWaitObserverForTest {
    pub async fn observe<F: Future>(mut self, future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        poll_fn(|cx| match future.as_mut().poll(cx) {
            Poll::Pending => {
                if let Some(event) = self.event.take() {
                    let _ = self.notify.send(event);
                }
                Poll::Pending
            }
            Poll::Ready(result) => {
                self.state.store(1, Ordering::Release);
                Poll::Ready(result)
            }
        })
        .await
    }
}

impl Drop for WebSocketReconnectWaitObserverForTest {
    fn drop(&mut self) {
        let _ = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// Handshake observer that first parks on a test-held admission semaphore, so
/// tests can hold the live reconnect at the deterministic boundary before the
/// websocket handshake starts, then release it.
pub(crate) struct WebSocketReconnectHandshakeObserverForTest {
    pub(super) wait: WebSocketReconnectWaitObserverForTest,
    pub(super) admission: Arc<Semaphore>,
}

impl WebSocketReconnectHandshakeObserverForTest {
    pub async fn observe<F: Future>(self, future: F) -> F::Output {
        let admission = self.admission.clone();
        let admission_permit = admission.acquire_owned().await;
        if let Ok(permit) = admission_permit {
            // The permit is consumed by the gate: releasing it here would let a
            // second handshake through on the same release.
            std::mem::forget(permit);
        }
        self.wait.observe(future).await
    }
}

/// Executor-side half of an installed observation: the event channel and the
/// handshake admission gate.
#[derive(Clone)]
pub struct WebSocketReconnectObservationInnerForTest {
    pub(crate) notify: mpsc::UnboundedSender<WebSocketReconnectEventForTest>,
    pub(crate) handshake_release: Arc<Semaphore>,
}

impl WebSocketReconnectObservationInnerForTest {
    pub(crate) fn new(
        notify: mpsc::UnboundedSender<WebSocketReconnectEventForTest>,
        handshake_release: Arc<Semaphore>,
    ) -> Self {
        Self {
            notify,
            handshake_release,
        }
    }
}

/// Test-side handle for an installed observation.
pub struct WebSocketReconnectObservationForTest {
    pub events: mpsc::UnboundedReceiver<WebSocketReconnectEventForTest>,
    pub control: WebSocketReconnectObservationControlForTest,
}

/// Test-side controls for an installed observation.
pub struct WebSocketReconnectObservationControlForTest {
    handshake_release: Arc<Semaphore>,
}

impl WebSocketReconnectObservationControlForTest {
    /// Releases the next held websocket reconnect handshake.
    pub fn release_handshake(&self) {
        self.handshake_release.add_permits(1);
    }
}

impl WebSocketReconnectObservationForTest {
    pub fn new() -> (Self, WebSocketReconnectObservationInnerForTest) {
        let (notify, events) = mpsc::unbounded_channel();
        let handshake_release = Arc::new(Semaphore::new(0));
        (
            Self {
                events,
                control: WebSocketReconnectObservationControlForTest {
                    handshake_release: handshake_release.clone(),
                },
            },
            WebSocketReconnectObservationInnerForTest::new(notify, handshake_release),
        )
    }
}
