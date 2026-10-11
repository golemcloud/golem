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
use golem_common::model::IdempotencyKey;
use golem_common::model::oplog::OplogIndex;
use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Poll;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P2NativeInputStateForTest {
    Pending,
    Returned,
    Dropped,
}

/// Identifies a real pending host wait and its permit-held runtime without the Store lock.
/// HTTP body waits also carry their durable child Start index.
#[derive(Debug)]
pub struct P2NativeInputPendingForTest {
    pub invocation_key: Option<IdempotencyKey>,
    pub runtime: MonthlyProposalForTest,
    pub stream_rep: u32,
    pub operation: &'static str,
    pub start_index: Option<OplogIndex>,
    pub output_capacity: Option<u64>,
    state: Arc<AtomicU8>,
}

impl P2NativeInputPendingForTest {
    pub fn state(&self) -> P2NativeInputStateForTest {
        match self.state.load(Ordering::Acquire) {
            0 => P2NativeInputStateForTest::Pending,
            1 => P2NativeInputStateForTest::Returned,
            2 => P2NativeInputStateForTest::Dropped,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
pub struct P2SplicePreSubscriptionEnteredForTest {
    pub invocation_key: IdempotencyKey,
    pub operation: &'static str,
    pub input_stream_rep: u32,
    pub output_stream_rep: u32,
    pub output_capacity: Option<u64>,
    pub runtime: MonthlyProposalForTest,
}

pub struct P2SplicePreSubscriptionControlForTest {
    pub entered: oneshot::Receiver<P2SplicePreSubscriptionEnteredForTest>,
    release: Option<oneshot::Sender<()>>,
}

impl P2SplicePreSubscriptionControlForTest {
    pub fn release(&mut self) {
        self.release.take();
    }
}

pub(crate) struct P2SplicePreSubscriptionGate {
    key: IdempotencyKey,
    entered: oneshot::Sender<P2SplicePreSubscriptionEnteredForTest>,
    release: oneshot::Receiver<()>,
}

pub(crate) struct P2NativeInputObserverForTest {
    notify: mpsc::UnboundedSender<P2NativeInputPendingForTest>,
    event: Option<P2NativeInputPendingForTest>,
    state: Arc<AtomicU8>,
}

impl P2NativeInputObserverForTest {
    pub(crate) fn set_output_capacity(&mut self, capacity: u64) {
        if let Some(event) = &mut self.event {
            event.output_capacity = Some(capacity);
        }
    }

    /// Poll exactly the supplied future. No artificial readiness, gate or wakeup.
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

impl Drop for P2NativeInputObserverForTest {
    fn drop(&mut self) {
        let _ = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub fn pause_next_p2_splice_pre_subscription_for_test(
        &self,
        key: IdempotencyKey,
    ) -> P2SplicePreSubscriptionControlForTest {
        let (entered_tx, entered) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .p2_splice_pre_subscription_gate
                .replace(P2SplicePreSubscriptionGate {
                    key,
                    entered: entered_tx,
                    release: release_rx,
                })
                .is_none()
        );
        P2SplicePreSubscriptionControlForTest {
            entered,
            release: Some(release),
        }
    }

    pub(crate) async fn wait_p2_splice_pre_subscription_for_test(
        &self,
        key: Option<&IdempotencyKey>,
        input_stream_rep: u32,
        output_stream_rep: u32,
        output_capacity: Option<u64>,
    ) {
        let gate = {
            let mut progress = self.stop_progress.lock().unwrap();
            let matches = progress
                .p2_splice_pre_subscription_gate
                .as_ref()
                .is_some_and(|gate| Some(&gate.key) == key);
            matches.then(|| progress.p2_splice_pre_subscription_gate.take().unwrap())
        };
        if let Some(gate) = gate {
            let _ = gate.entered.send(P2SplicePreSubscriptionEnteredForTest {
                invocation_key: gate.key,
                operation: "blocking_splice",
                input_stream_rep,
                output_stream_rep,
                output_capacity,
                runtime: self.current_monthly_proposal_for_test(),
            });
            let _ = gate.release.await;
        }
    }

    pub fn observe_p2_native_input_for_test(
        &self,
    ) -> mpsc::UnboundedReceiver<P2NativeInputPendingForTest> {
        self.observe_p2_native_input_with_retry_delay_for_test(false)
    }

    pub fn observe_p2_native_input_with_retry_delay_for_test(
        &self,
        include_retry_delay: bool,
    ) -> mpsc::UnboundedReceiver<P2NativeInputPendingForTest> {
        let (notify, observe) = mpsc::unbounded_channel();
        let mut progress = self.stop_progress.lock().unwrap();
        assert!(progress.p2_native_input_observer.replace(notify).is_none());
        progress.p2_native_retry_delay_observed = include_retry_delay;
        observe
    }

    pub(crate) fn p2_native_input_observer_for_test(
        &self,
        invocation_key: Option<IdempotencyKey>,
        stream_rep: u32,
        operation: &'static str,
        start_index: Option<OplogIndex>,
    ) -> Option<P2NativeInputObserverForTest> {
        let notify = {
            let progress = self.stop_progress.lock().unwrap();
            if operation == "http_body_retry_delay_sleep"
                && !progress.p2_native_retry_delay_observed
            {
                return None;
            }
            progress.p2_native_input_observer.clone()?
        };
        let state = Arc::new(AtomicU8::new(0));
        Some(P2NativeInputObserverForTest {
            notify,
            event: Some(P2NativeInputPendingForTest {
                invocation_key,
                runtime: self.current_monthly_proposal_for_test(),
                stream_rep,
                operation,
                start_index,
                output_capacity: None,
                state: state.clone(),
            }),
            state,
        })
    }
}
