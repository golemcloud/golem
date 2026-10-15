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
pub enum P2PollStateForTest {
    Pending,
    Returned,
    Dropped,
}

/// The first native poll Pending and its current state, without acquiring the Store lock.
#[derive(Debug)]
pub struct P2PollPendingForTest {
    pub invocation_key: Option<IdempotencyKey>,
    pub runtime: MonthlyProposalForTest,
    pub start: OplogIndex,
    pub pollable_reps: Vec<u32>,
    state: Arc<AtomicU8>,
}

impl P2PollPendingForTest {
    pub fn state(&self) -> P2PollStateForTest {
        match self.state.load(Ordering::Acquire) {
            0 => P2PollStateForTest::Pending,
            1 => P2PollStateForTest::Returned,
            2 => P2PollStateForTest::Dropped,
            _ => unreachable!(),
        }
    }
}

pub(crate) struct P2PollObserverForTest {
    notify: mpsc::UnboundedSender<P2PollPendingForTest>,
    event: Option<P2PollPendingForTest>,
    state: Arc<AtomicU8>,
}

impl P2PollObserverForTest {
    /// Poll exactly the supplied native future. No artificial readiness, gate or wakeup.
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

impl Drop for P2PollObserverForTest {
    fn drop(&mut self) {
        let _ = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub fn observe_p2_poll_for_test(&self) -> mpsc::UnboundedReceiver<P2PollPendingForTest> {
        let (notify, observe) = mpsc::unbounded_channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .p2_poll_observer
                .replace(notify)
                .is_none()
        );
        observe
    }

    pub(crate) fn p2_poll_observer_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        start: OplogIndex,
        pollable_reps: Vec<u32>,
    ) -> Option<P2PollObserverForTest> {
        let notify = self
            .stop_progress
            .lock()
            .unwrap()
            .p2_poll_observer
            .clone()?;
        let state = Arc::new(AtomicU8::new(0));
        Some(P2PollObserverForTest {
            notify,
            event: Some(P2PollPendingForTest {
                invocation_key,
                runtime: self.current_monthly_proposal_for_test(),
                start,
                pollable_reps,
                state: state.clone(),
            }),
            state,
        })
    }
}
