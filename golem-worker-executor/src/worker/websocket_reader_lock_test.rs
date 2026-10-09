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
use anyhow::{Context as _, ensure};
use golem_common::model::{IdempotencyKey, OplogIndex};
use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Poll;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Debug)]
pub struct WebSocketReaderBoundaryForTest {
    pub invocation_key: IdempotencyKey,
    pub start: OplogIndex,
    pub connection_rep: u32,
    pub runtime: MonthlyProposalForTest,
}

pub(super) struct WebSocketReaderOrderForTest {
    key: IdempotencyKey,
    first_pending: Option<(
        oneshot::Sender<WebSocketReaderBoundaryForTest>,
        oneshot::Sender<WebSocketReaderBoundaryForTest>,
    )>,
    timed_start: Option<oneshot::Receiver<WebSocketReaderBoundaryForTest>>,
    timed_gate: Option<(
        oneshot::Sender<WebSocketReaderBoundaryForTest>,
        oneshot::Receiver<()>,
    )>,
}

impl WebSocketReaderOrderForTest {
    fn take_guard(
        &mut self,
        key: Option<&IdempotencyKey>,
    ) -> Option<(
        oneshot::Sender<WebSocketReaderBoundaryForTest>,
        oneshot::Sender<WebSocketReaderBoundaryForTest>,
    )> {
        (key == Some(&self.key))
            .then(|| self.first_pending.take())
            .flatten()
    }

    fn take_start(
        &mut self,
        is_live: bool,
        key: Option<&IdempotencyKey>,
    ) -> Option<oneshot::Receiver<WebSocketReaderBoundaryForTest>> {
        (is_live && key == Some(&self.key))
            .then(|| self.timed_start.take())
            .flatten()
    }

    fn take_timed(
        &mut self,
        key: Option<&IdempotencyKey>,
    ) -> Option<(
        oneshot::Sender<WebSocketReaderBoundaryForTest>,
        oneshot::Receiver<()>,
    )> {
        (key == Some(&self.key))
            .then(|| self.timed_gate.take())
            .flatten()
    }
}

pub(crate) struct WebSocketReaderGuardObserverForTest {
    entered: oneshot::Sender<WebSocketReaderBoundaryForTest>,
    start_after_pending: oneshot::Sender<WebSocketReaderBoundaryForTest>,
    event: WebSocketReaderBoundaryForTest,
}

async fn observe_first_pending<F: Future>(future: F, entered: impl FnOnce()) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut entered = Some(entered);
    poll_fn(|cx| {
        let polled = future.as_mut().poll(cx);
        if polled.is_pending()
            && let Some(entered) = entered.take()
        {
            entered();
        }
        polled
    })
    .await
}

impl WebSocketReaderGuardObserverForTest {
    pub async fn observe<F: Future>(self, future: F) -> F::Output {
        observe_first_pending(future, || {
            let _ = self.start_after_pending.send(self.event.clone());
            let _ = self.entered.send(self.event);
        })
        .await
    }
}

pub(crate) struct WebSocketTimedReaderStartGateForTest {
    first_pending: oneshot::Receiver<WebSocketReaderBoundaryForTest>,
    invocation_key: IdempotencyKey,
    connection_rep: u32,
    runtime: MonthlyProposalForTest,
}

impl WebSocketTimedReaderStartGateForTest {
    pub async fn wait(self) -> anyhow::Result<()> {
        let boundary = self
            .first_pending
            .await
            .context("untimed frame observer closed before first Pending")?;
        ensure!(boundary.invocation_key == self.invocation_key);
        ensure!(boundary.connection_rep == self.connection_rep);
        ensure!(boundary.runtime == self.runtime);
        Ok(())
    }
}

pub(crate) struct WebSocketTimedReaderGateForTest {
    entered: oneshot::Sender<WebSocketReaderBoundaryForTest>,
    release: oneshot::Receiver<()>,
    event: WebSocketReaderBoundaryForTest,
}

impl WebSocketTimedReaderGateForTest {
    pub async fn wait(self) {
        let _ = self.entered.send(self.event);
        let _ = self.release.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentFingerprint;
    use golem_common::model::account_usage::AccountUsagePeriod;
    use std::sync::atomic::AtomicUsize;
    use test_r::test;
    use uuid::Uuid;

    fn boundary() -> WebSocketReaderBoundaryForTest {
        WebSocketReaderBoundaryForTest {
            invocation_key: IdempotencyKey::fresh(),
            start: OplogIndex::INITIAL,
            connection_rep: 1,
            runtime: MonthlyProposalForTest {
                fingerprint: AgentFingerprint::new(),
                start_attempt: Uuid::new_v4(),
                resident_generation: 1,
                window_identity: 1,
                policy_revision: 1,
                period: AccountUsagePeriod {
                    year: 2026,
                    month: 1,
                },
                fuel_generation: None,
                exhaustion: None,
            },
        }
    }

    fn reader_gates(
        event: WebSocketReaderBoundaryForTest,
    ) -> (
        WebSocketReaderGuardObserverForTest,
        oneshot::Receiver<WebSocketReaderBoundaryForTest>,
        WebSocketTimedReaderStartGateForTest,
    ) {
        let (entered, observable) = oneshot::channel();
        let (start_after_pending, first_pending) = oneshot::channel();
        let start_gate = WebSocketTimedReaderStartGateForTest {
            first_pending,
            invocation_key: event.invocation_key.clone(),
            connection_rep: event.connection_rep,
            runtime: event.runtime,
        };
        (
            WebSocketReaderGuardObserverForTest {
                entered,
                start_after_pending,
                event,
            },
            observable,
            start_gate,
        )
    }

    #[test]
    async fn first_pending_observer_does_not_report_ready_and_reports_pending_once() {
        let observed = AtomicUsize::new(0);
        observe_first_pending(std::future::ready(()), || {
            observed.fetch_add(1, Ordering::Relaxed);
        })
        .await;
        assert_eq!(observed.load(Ordering::Relaxed), 0);
        let (send, receive) = oneshot::channel::<()>();
        let future = observe_first_pending(receive, || {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        tokio::pin!(future);
        assert!(futures::poll!(&mut future).is_pending());
        assert!(futures::poll!(&mut future).is_pending());
        assert_eq!(observed.load(Ordering::Relaxed), 1);
        send.send(()).unwrap();
        future.await.unwrap();
        assert_eq!(observed.load(Ordering::Relaxed), 1);

        let (observer, observable, start_gate) = reader_gates(boundary());
        observer.observe(std::future::ready(())).await;
        assert!(observable.await.is_err());
        assert!(start_gate.wait().await.is_err());

        let (observer, observable, start_gate) = reader_gates(boundary());
        let unpolled = observer.observe(std::future::pending::<()>());
        drop(unpolled);
        assert!(observable.await.is_err());
        assert!(start_gate.wait().await.is_err());

        let (observer, observable, start_gate) = reader_gates(boundary());
        drop(observer);
        assert!(observable.await.is_err());
        assert!(start_gate.wait().await.is_err());

        let event = boundary();
        let (observer, observable, start_gate) = reader_gates(event.clone());
        let (send, receive) = oneshot::channel::<()>();
        let future = observer.observe(receive);
        tokio::pin!(future);
        assert!(futures::poll!(&mut future).is_pending());
        assert!(futures::poll!(&mut future).is_pending());
        let received = observable.await.unwrap();
        assert_eq!(received.invocation_key, event.invocation_key);
        assert_eq!(received.start, event.start);
        assert_eq!(received.connection_rep, event.connection_rep);
        assert_eq!(received.runtime, event.runtime);
        start_gate.wait().await.unwrap();
        send.send(()).unwrap();
        future.await.unwrap();
    }

    #[test]
    async fn reader_order_gates_take_only_the_exact_invocation_once() {
        let key = IdempotencyKey::fresh();
        let (native, _) = oneshot::channel();
        let (start_after_pending, timed_start) = oneshot::channel();
        let (timed, _) = oneshot::channel();
        let (_, wait) = oneshot::channel();
        let mut order = WebSocketReaderOrderForTest {
            key: key.clone(),
            first_pending: Some((native, start_after_pending)),
            timed_start: Some(timed_start),
            timed_gate: Some((timed, wait)),
        };
        let other = IdempotencyKey::fresh();
        assert!(order.take_start(false, Some(&key)).is_none());
        assert!(order.take_start(true, None).is_none());
        assert!(order.take_start(true, Some(&other)).is_none());
        assert!(order.take_guard(None).is_none());
        assert!(order.take_guard(Some(&other)).is_none());
        assert!(order.take_timed(Some(&other)).is_none());
        assert!(order.first_pending.is_some());
        assert!(order.timed_start.is_some());
        assert!(order.timed_gate.is_some());
        assert!(order.take_start(true, Some(&key)).is_some());
        assert!(order.take_start(true, Some(&key)).is_none());
        assert!(order.take_guard(Some(&key)).is_some());
        assert!(order.take_guard(Some(&key)).is_none());
        assert!(order.take_timed(Some(&key)).is_some());
        assert!(order.take_timed(Some(&key)).is_none());

        for mismatch in 0..3 {
            let expected = boundary();
            let mut actual = expected.clone();
            match mismatch {
                0 => actual.invocation_key = IdempotencyKey::fresh(),
                1 => actual.connection_rep += 1,
                2 => actual.runtime.window_identity += 1,
                _ => unreachable!(),
            }
            let (send, first_pending) = oneshot::channel();
            let gate = WebSocketTimedReaderStartGateForTest {
                first_pending,
                invocation_key: expected.invocation_key,
                connection_rep: expected.connection_rep,
                runtime: expected.runtime,
            };
            send.send(actual).unwrap();
            assert!(gate.wait().await.is_err());
        }
    }
}

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
    pub connection_rep: u32,
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
    pub fn order_websocket_readers_for_test(
        &self,
        key: IdempotencyKey,
    ) -> (
        oneshot::Receiver<WebSocketReaderBoundaryForTest>,
        oneshot::Receiver<WebSocketReaderBoundaryForTest>,
        oneshot::Sender<()>,
    ) {
        let (native, pending) = oneshot::channel();
        let (start_after_pending, timed_start) = oneshot::channel();
        let (timed, entered) = oneshot::channel();
        let (release, wait) = oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .websocket_reader_order
                .replace(WebSocketReaderOrderForTest {
                    key,
                    first_pending: Some((native, start_after_pending)),
                    timed_start: Some(timed_start),
                    timed_gate: Some((timed, wait)),
                })
                .is_none()
        );
        (pending, entered, release)
    }

    pub(crate) fn websocket_reader_guard_observer_for_test(
        &self,
        key: Option<IdempotencyKey>,
        start: OplogIndex,
        connection_rep: u32,
    ) -> Option<WebSocketReaderGuardObserverForTest> {
        let mut progress = self.stop_progress.lock().unwrap();
        let order = progress.websocket_reader_order.as_mut()?;
        let (entered, start_after_pending) = order.take_guard(key.as_ref())?;
        let invocation_key = order.key.clone();
        drop(progress);
        Some(WebSocketReaderGuardObserverForTest {
            entered,
            start_after_pending,
            event: WebSocketReaderBoundaryForTest {
                invocation_key,
                start,
                connection_rep,
                runtime: self.current_monthly_proposal_for_test(),
            },
        })
    }

    pub(crate) fn websocket_timed_reader_start_gate_for_test(
        &self,
        is_live: bool,
        key: Option<IdempotencyKey>,
        connection_rep: u32,
    ) -> Option<WebSocketTimedReaderStartGateForTest> {
        let mut progress = self.stop_progress.lock().unwrap();
        let order = progress.websocket_reader_order.as_mut()?;
        let first_pending = order.take_start(is_live, key.as_ref())?;
        let invocation_key = order.key.clone();
        drop(progress);
        Some(WebSocketTimedReaderStartGateForTest {
            first_pending,
            invocation_key,
            connection_rep,
            runtime: self.current_monthly_proposal_for_test(),
        })
    }

    pub(crate) fn websocket_timed_reader_gate_for_test(
        &self,
        key: Option<IdempotencyKey>,
        start: OplogIndex,
        connection_rep: u32,
    ) -> Option<WebSocketTimedReaderGateForTest> {
        let mut progress = self.stop_progress.lock().unwrap();
        let order = progress.websocket_reader_order.as_mut()?;
        let (entered, release) = order.take_timed(key.as_ref())?;
        let invocation_key = order.key.clone();
        drop(progress);
        Some(WebSocketTimedReaderGateForTest {
            entered,
            release,
            event: WebSocketReaderBoundaryForTest {
                invocation_key,
                start,
                connection_rep,
                runtime: self.current_monthly_proposal_for_test(),
            },
        })
    }

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
        connection_rep: u32,
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
                connection_rep,
                runtime: self.current_monthly_proposal_for_test(),
                state: state.clone(),
            }),
            state,
        })
    }
}
