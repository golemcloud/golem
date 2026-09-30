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
pub enum WebSocketTimedReceiveStateForTest {
    Pending,
    Returned,
    Dropped,
}

#[derive(Debug)]
pub struct WebSocketTimedReceivePendingForTest {
    pub invocation_key: Option<IdempotencyKey>,
    pub start: OplogIndex,
    pub runtime: MonthlyProposalForTest,
    state: Arc<AtomicU8>,
}

impl WebSocketTimedReceivePendingForTest {
    pub fn state(&self) -> WebSocketTimedReceiveStateForTest {
        match self.state.load(Ordering::Acquire) {
            0 => WebSocketTimedReceiveStateForTest::Pending,
            1 => WebSocketTimedReceiveStateForTest::Returned,
            2 => WebSocketTimedReceiveStateForTest::Dropped,
            _ => unreachable!(),
        }
    }
}

pub(crate) struct WebSocketTimedReceiveObserverForTest {
    notify: mpsc::UnboundedSender<WebSocketTimedReceivePendingForTest>,
    event: Option<WebSocketTimedReceivePendingForTest>,
    state: Arc<AtomicU8>,
}

impl WebSocketTimedReceiveObserverForTest {
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

impl Drop for WebSocketTimedReceiveObserverForTest {
    fn drop(&mut self) {
        let _ = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub fn observe_websocket_timed_receive_for_test(
        &self,
    ) -> mpsc::UnboundedReceiver<WebSocketTimedReceivePendingForTest> {
        let (notify, observe) = mpsc::unbounded_channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .websocket_timed_receive_observer
                .replace(notify)
                .is_none()
        );
        observe
    }

    pub(crate) fn websocket_timed_receive_observer_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
    ) -> Option<WebSocketTimedReceiveObserverForTest> {
        let notify = self
            .stop_progress
            .lock()
            .unwrap()
            .websocket_timed_receive_observer
            .clone()?;
        let state = Arc::new(AtomicU8::new(0));
        Some(WebSocketTimedReceiveObserverForTest {
            notify,
            event: Some(WebSocketTimedReceivePendingForTest {
                invocation_key,
                start,
                runtime: self.current_monthly_proposal_for_test(),
                state: state.clone(),
            }),
            state,
        })
    }
}
