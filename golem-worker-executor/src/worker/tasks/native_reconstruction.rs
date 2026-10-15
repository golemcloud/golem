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

use super::{JoinedChildren, TaskScope, WorkerTasks};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::{Future, poll_fn};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

/// Reconstruction belongs to the native body, not to an individual attachment-call future.
/// Dropping this owner revokes escaped spawners and transfers abort/join before releasing the
/// worker task scope. Neither a spawner nor an active monitor keeps the body alive.
pub(crate) struct NativeReconstructionTasks {
    shared: Arc<Mutex<State>>,
    _scope: TaskScope,
}

#[derive(Clone)]
pub(crate) struct NativeReconstructionSpawner(Weak<Mutex<State>>);

/// Drive this monitor alongside the handler, then drop its wait before sealing and draining.
/// Successful children are reaped without completing the wait; the first failure is retained
/// independently of any operation-specific receipt.
pub(crate) struct NativeReconstructionMonitor(Weak<Mutex<State>>);

struct State {
    children: Option<JoinedChildren<Result<(), WorkerExecutorError>>>,
    sealed: bool,
    failure: Option<WorkerExecutorError>,
    monitor: Option<Waker>,
    body_failure: Option<WorkerExecutorError>,
    body_monitor: Option<Waker>,
}

fn closed() -> WorkerExecutorError {
    WorkerExecutorError::runtime("native reconstruction task owner is closed")
}

impl NativeReconstructionTasks {
    pub(crate) fn new(tasks: &WorkerTasks) -> Result<Self, WorkerExecutorError> {
        let scope = TaskScope::default();
        scope.bind(tasks).map_err(WorkerExecutorError::runtime)?;
        Ok(Self {
            shared: Arc::new(Mutex::new(State {
                children: Some(tasks.children()),
                sealed: false,
                failure: None,
                monitor: None,
                body_failure: None,
                body_monitor: None,
            })),
            _scope: scope,
        })
    }

    pub(crate) fn spawner(&self) -> NativeReconstructionSpawner {
        NativeReconstructionSpawner(Arc::downgrade(&self.shared))
    }

    pub(crate) fn monitor(&self) -> NativeReconstructionMonitor {
        NativeReconstructionMonitor(Arc::downgrade(&self.shared))
    }

    /// Shut down admission after a body failure without replacing its original error.
    /// Accepted publications have separate owners and are not children of this set.
    pub(crate) async fn abort_and_join(&mut self) {
        let (children, monitor, body_monitor) = {
            let mut state = self.shared.lock().unwrap();
            state.sealed = true;
            (
                state.children.take(),
                state.monitor.take(),
                state.body_monitor.take(),
            )
        };
        if let Some(monitor) = monitor {
            monitor.wake();
        }
        if let Some(monitor) = body_monitor {
            monitor.wake();
        }
        if let Some(mut children) = children {
            children.abort_all();
            while children.join_next().await.is_some() {}
        }
    }

    pub(crate) async fn seal_and_drain(&mut self) -> Result<(), WorkerExecutorError> {
        self.shared.lock().unwrap().sealed = true;
        poll_fn(|cx| {
            let mut state = self.shared.lock().unwrap();
            state.poll_children(cx).map(|()| match &state.failure {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            })
        })
        .await
    }
}

impl Drop for NativeReconstructionTasks {
    fn drop(&mut self) {
        let (children, monitor, body_monitor) = {
            let mut state = self.shared.lock().unwrap();
            state.sealed = true;
            (
                state.children.take(),
                state.monitor.take(),
                state.body_monitor.take(),
            )
        };
        // JoinedChildren transfers joins while this owner's TaskScope still holds its token.
        drop(children);
        if let Some(monitor) = monitor {
            monitor.wake();
        }
        if let Some(monitor) = body_monitor {
            monitor.wake();
        }
    }
}

impl NativeReconstructionSpawner {
    /// A body outcome is handled by entity finalization. Unlike reconstruction failure, it
    /// must not abort accepted writes that finalization still needs to drain.
    pub(crate) fn report_body_failure(
        &self,
        failure: impl Future<Output = WorkerExecutorError> + Send + 'static,
    ) -> Result<(), WorkerExecutorError> {
        let shared = self.0.clone();
        self.spawn(async move {
            let error = failure.await;
            if let Some(shared) = shared.upgrade() {
                let monitor = {
                    let mut state = shared.lock().unwrap();
                    if state.body_failure.is_none() {
                        state.body_failure = Some(error);
                    }
                    state.body_monitor.take()
                };
                if let Some(monitor) = monitor {
                    monitor.wake();
                }
            }
            Ok(())
        })
    }

    pub(crate) fn spawn(
        &self,
        future: impl Future<Output = Result<(), WorkerExecutorError>> + Send + 'static,
    ) -> Result<(), WorkerExecutorError> {
        let shared = self.0.upgrade().ok_or_else(closed)?;
        let monitor = {
            let mut state = shared.lock().unwrap();
            if state.sealed {
                return Err(state.failure.clone().unwrap_or_else(closed));
            }
            state.children.as_mut().ok_or_else(closed)?.spawn(future);
            state.monitor.take()
        };
        if let Some(monitor) = monitor {
            monitor.wake();
        }
        Ok(())
    }
}

impl NativeReconstructionMonitor {
    /// Join the currently registered work without closing admission for the next invocation.
    /// The context owner still revokes this weak handle and aborts children on destruction.
    pub(crate) async fn drain(&self) -> Result<(), WorkerExecutorError> {
        poll_fn(|cx| {
            let Some(shared) = self.0.upgrade() else {
                return Poll::Ready(Err(closed()));
            };
            let mut state = shared.lock().unwrap();
            if state.children.is_none() {
                return Poll::Ready(Err(closed()));
            }
            state.poll_children(cx).map(|()| match &state.failure {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            })
        })
        .await
    }

    pub(crate) fn take_body_failure(&self) -> Option<WorkerExecutorError> {
        self.0.upgrade()?.lock().unwrap().body_failure.take()
    }

    pub(crate) async fn wait_for_body_failure(&self) -> WorkerExecutorError {
        poll_fn(|cx| {
            let Some(shared) = self.0.upgrade() else {
                return Poll::Ready(closed());
            };
            let mut state = shared.lock().unwrap();
            if let Some(error) = state.body_failure.take() {
                Poll::Ready(error)
            } else if state.children.is_none() {
                Poll::Ready(closed())
            } else {
                state.body_monitor = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    pub(crate) async fn wait_for_failure(&self) -> WorkerExecutorError {
        poll_fn(|cx| {
            let Some(shared) = self.0.upgrade() else {
                return Poll::Ready(closed());
            };
            let mut state = shared.lock().unwrap();
            if state.children.is_none() {
                return Poll::Ready(closed());
            }
            let _ = state.poll_children(cx);
            if let Some(error) = &state.failure {
                Poll::Ready(error.clone())
            } else {
                // JoinSet has no registered waiter when it is empty; spawn must wake us too.
                state.monitor = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

impl State {
    fn poll_children(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(children) = &mut self.children else {
            return Poll::Ready(());
        };
        loop {
            let result = match children.poll_join_next(cx) {
                Poll::Ready(Some(result)) => result,
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => return Poll::Pending,
            };
            let error = match result {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => error,
                Err(error) => WorkerExecutorError::runtime(format!(
                    "native reconstruction task failed: {error}"
                )),
            };
            if self.failure.is_none() {
                self.failure = Some(error);
                self.sealed = true;
                children.abort_all();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::{test, timeout};
    use tokio::sync::oneshot;

    struct SignalOnDrop(Option<oneshot::Sender<()>>);

    impl Drop for SignalOnDrop {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }

    #[test]
    #[timeout("30s")]
    async fn aborted_admission_joins_without_waiting_for_replay_progress() {
        let worker = WorkerTasks::default();
        let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
        let spawner = owner.spawner();
        let (dropped, observed_drop) = oneshot::channel();
        let guard = SignalOnDrop(Some(dropped));
        spawner
            .spawn(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
                Ok(())
            })
            .unwrap();
        owner.abort_and_join().await;
        observed_drop.await.unwrap();
        assert!(spawner.spawn(async { Ok(()) }).is_err());
        drop(owner);
        worker.stop_and_wait().await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn reusable_drain_joins_registered_work_before_first_poll_and_after_wakeup() {
        let worker = WorkerTasks::default();
        let owner = NativeReconstructionTasks::new(&worker).unwrap();
        let monitor = owner.monitor();
        for _ in 0..2 {
            let (release, released) = oneshot::channel();
            let (handoff, received) = oneshot::channel();
            owner
                .spawner()
                .spawn(async move {
                    released.await.unwrap();
                    handoff.send(()).unwrap();
                    Ok(())
                })
                .unwrap();
            let drain = monitor.drain();
            tokio::pin!(drain);
            assert!(futures::poll!(drain.as_mut()).is_pending());
            release.send(()).unwrap();
            drain.await.unwrap();
            received.await.unwrap();
        }
        owner
            .spawner()
            .spawn(async { Err(WorkerExecutorError::runtime("admission interrupted")) })
            .unwrap();
        let failure = monitor.drain().await.unwrap_err();
        assert_eq!(
            failure,
            WorkerExecutorError::runtime("admission interrupted")
        );
        assert_eq!(monitor.drain().await.unwrap_err(), failure);
        drop(owner);
        worker.stop_and_wait().await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn cancelled_operation_does_not_cancel_body_reconstruction() {
        let worker = WorkerTasks::default();
        let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
        let spawner = owner.spawner();
        let (release, released) = oneshot::channel();
        let (receipt, operation) = oneshot::channel();
        let restored = Arc::new(AtomicBool::new(false));
        let effect = restored.clone();
        spawner
            .spawn(async move {
                released.await.unwrap();
                effect.store(true, Ordering::SeqCst);
                let _ = receipt.send(());
                Ok(())
            })
            .unwrap();
        drop(operation);
        {
            let drain = owner.seal_and_drain();
            tokio::pin!(drain);
            assert!(futures::poll!(drain.as_mut()).is_pending());
            assert!(spawner.spawn(async { Ok(()) }).is_err());
            assert!(!restored.load(Ordering::SeqCst));
            release.send(()).unwrap();
            drain.await.unwrap();
        }
        assert!(restored.load(Ordering::SeqCst));
        drop(owner);
        worker.stop_and_wait().await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn body_failure_reporting_does_not_abort_or_hide_reconstruction() {
        for fail_restore in [false, true] {
            let worker = WorkerTasks::default();
            let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
            let spawner = owner.spawner();
            let monitor = owner.monitor();
            let (release, released) = oneshot::channel();
            let restored = Arc::new(AtomicBool::new(false));
            let effect = restored.clone();
            spawner
                .spawn(async move {
                    released.await.unwrap();
                    effect.store(true, Ordering::SeqCst);
                    if fail_restore {
                        Err(WorkerExecutorError::runtime("restore failed"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap();
            let admission = crate::durable_host::tool_attachment_live_admission_rejected_error();
            let reported = admission.clone();
            spawner
                .report_body_failure(async move { reported })
                .unwrap();
            assert_eq!(monitor.wait_for_body_failure().await, admission);
            assert!(monitor.take_body_failure().is_none());
            assert!(!restored.load(Ordering::SeqCst));
            release.send(()).unwrap();
            let result = owner.seal_and_drain().await;
            assert!(restored.load(Ordering::SeqCst));
            if fail_restore {
                assert_eq!(
                    result.unwrap_err(),
                    WorkerExecutorError::runtime("restore failed")
                );
            } else {
                result.unwrap();
            }
            drop(owner);
            worker.stop_and_wait().await.unwrap();
        }

        let worker = WorkerTasks::default();
        let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
        let monitor = owner.monitor();
        let (release, released) = oneshot::channel();
        owner
            .spawner()
            .report_body_failure(async move {
                released.await.unwrap();
                WorkerExecutorError::runtime("late body failure")
            })
            .unwrap();
        {
            let drain = owner.seal_and_drain();
            tokio::pin!(drain);
            assert!(futures::poll!(drain.as_mut()).is_pending());
            release.send(()).unwrap();
            drain.await.unwrap();
        }
        assert_eq!(
            monitor.take_body_failure(),
            Some(WorkerExecutorError::runtime("late body failure"))
        );
        drop(owner);
        worker.stop_and_wait().await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn active_monitor_retains_failure_and_panic_without_operation_observer() {
        for panic in [false, true] {
            let worker = WorkerTasks::default();
            let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
            let spawner = owner.spawner();
            let monitor = owner.monitor();
            let (parked, parked_rx) = oneshot::channel();
            let failure = tokio::spawn(async move {
                let failure = monitor.wait_for_failure();
                tokio::pin!(failure);
                let mut parked = Some(parked);
                poll_fn(|cx| {
                    let result = failure.as_mut().poll(cx);
                    if result.is_pending()
                        && let Some(parked) = parked.take()
                    {
                        let _ = parked.send(());
                    }
                    result
                })
                .await
            });
            parked_rx.await.unwrap();
            let (receipt, operation) = oneshot::channel::<()>();
            drop(operation);
            spawner
                .spawn(async move {
                    drop(receipt);
                    if panic {
                        panic!("injected reconstruction panic");
                    }
                    Err(WorkerExecutorError::runtime(
                        "injected reconstruction failure",
                    ))
                })
                .unwrap();
            let error = failure.await.unwrap();
            assert!(error.to_string().contains(if panic {
                "injected reconstruction panic"
            } else {
                "injected reconstruction failure"
            }));
            assert!(spawner.spawn(async { Ok(()) }).is_err());
            assert_eq!(owner.seal_and_drain().await.unwrap_err(), error);
            drop(owner);
            worker.stop_and_wait().await.unwrap();
        }
    }

    #[test]
    #[timeout("30s")]
    async fn body_teardown_revokes_escaped_handles_and_joins_owned_cleanup() {
        let worker = WorkerTasks::default();
        let owner = NativeReconstructionTasks::new(&worker).unwrap();
        let spawner = owner.spawner();
        let monitor = owner.monitor();
        let (started, started_rx) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (finished, finished_rx) = oneshot::channel();
        let (child_dropped, child_dropped_rx) = oneshot::channel();
        let child_worker = worker.clone();
        spawner
            .spawn(async move {
                let _dropped = SignalOnDrop(Some(child_dropped));
                let task = tokio::spawn(async move {
                    started.send(()).unwrap();
                    released.await.unwrap();
                    finished.send(()).unwrap();
                });
                child_worker.finish_on_drop(task).await.unwrap();
                Ok(())
            })
            .unwrap();
        started_rx.await.unwrap();
        let failure = monitor.wait_for_failure();
        tokio::pin!(failure);
        assert!(futures::poll!(failure.as_mut()).is_pending());
        let stop = worker.stop_and_wait();
        tokio::pin!(stop);
        assert!(futures::poll!(stop.as_mut()).is_pending());
        assert!(NativeReconstructionTasks::new(&worker).is_err());
        drop(owner);
        assert!(spawner.spawn(async { Ok(()) }).is_err());
        let _closed = failure.await;
        child_dropped_rx.await.unwrap();
        assert!(futures::poll!(stop.as_mut()).is_pending());
        release.send(()).unwrap();
        stop.await.unwrap();
        finished_rx.await.unwrap();
        assert!(NativeReconstructionTasks::new(&worker).is_err());
    }
}
