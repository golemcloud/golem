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

use super::{MonthlyProposalForTest, Worker};
use crate::workerctx::WorkerCtx;
use golem_common::model::{IdempotencyKey, OplogIndex};
use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Poll;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketReaderLockStateForTest {
    Pending,
    Acquired,
    Dropped,
}

#[derive(Debug)]
pub struct WebSocketReaderLockPendingForTest {
    pub invocation_key: Option<IdempotencyKey>,
    pub start: OplogIndex,
    pub path: &'static str,
    pub runtime: MonthlyProposalForTest,
    state: Arc<AtomicU8>,
}

impl WebSocketReaderLockPendingForTest {
    pub fn state(&self) -> WebSocketReaderLockStateForTest {
        match self.state.load(Ordering::Acquire) {
            0 => WebSocketReaderLockStateForTest::Pending,
            1 => WebSocketReaderLockStateForTest::Acquired,
            2 => WebSocketReaderLockStateForTest::Dropped,
            _ => unreachable!(),
        }
    }
}

pub(crate) struct WebSocketReaderLockObserverForTest {
    notify: mpsc::UnboundedSender<WebSocketReaderLockPendingForTest>,
    event: Option<WebSocketReaderLockPendingForTest>,
    state: Arc<AtomicU8>,
}

impl WebSocketReaderLockObserverForTest {
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

impl Drop for WebSocketReaderLockObserverForTest {
    fn drop(&mut self) {
        let _ = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub fn observe_websocket_reader_lock_for_test(
        &self,
    ) -> mpsc::UnboundedReceiver<WebSocketReaderLockPendingForTest> {
        let (notify, observe) = mpsc::unbounded_channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .websocket_reader_lock_observer
                .replace(notify)
                .is_none()
        );
        observe
    }

    pub(crate) fn websocket_reader_lock_observer_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
    ) -> Option<WebSocketReaderLockObserverForTest> {
        let notify = self
            .stop_progress
            .lock()
            .unwrap()
            .websocket_reader_lock_observer
            .clone()?;
        let state = Arc::new(AtomicU8::new(0));
        Some(WebSocketReaderLockObserverForTest {
            notify,
            event: Some(WebSocketReaderLockPendingForTest {
                invocation_key,
                start,
                path: "timed receive reader.lock",
                runtime: self.current_monthly_proposal_for_test(),
                state: state.clone(),
            }),
            state,
        })
    }
}
