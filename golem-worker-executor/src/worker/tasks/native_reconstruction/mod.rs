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
mod tests;
