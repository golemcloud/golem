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

use super::state_actor::WorkerStateActorStop;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{Context, Poll};
use tokio::sync::Notify;
use tokio::task::{JoinError, JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

/// Task lifetime shared by every worker shell using the same open oplog generation.
#[derive(Clone, Default)]
pub struct WorkerTasks(Arc<Inner>, Option<Weak<dyn Send + Sync>>);

#[derive(Default)]
struct Inner {
    closed: Mutex<bool>,
    tasks: TaskTracker,
    stop: CancellationToken,
    metadata_closed: Mutex<bool>,
    metadata: TaskTracker,
    actors: Mutex<ActorOwners>,
}

#[derive(Default)]
struct ActorOwners {
    closed: bool,
    owners: Vec<Weak<WorkerStateActorStop>>,
}

impl WorkerTasks {
    pub(crate) fn register_actor(&self, actor: &Arc<WorkerStateActorStop>) {
        let mut actors = self.0.actors.lock().unwrap();
        assert!(
            !actors.closed,
            "cannot construct a worker on a stopped oplog"
        );
        actors.owners.retain(|owner| owner.strong_count() != 0);
        actors.owners.push(Arc::downgrade(actor));
    }

    pub(crate) fn for_worker(&self, worker: Arc<dyn Send + Sync>) -> Self {
        Self(self.0.clone(), Some(Arc::downgrade(&worker)))
    }

    fn pin_worker(&self) -> Option<Arc<dyn Send + Sync>> {
        self.1.as_ref().and_then(Weak::upgrade)
    }

    /// Closes root admission before cancellation. Already admitted roots may transfer child
    /// joins from their destructors; their tracker tokens keep the wait open until that handoff.
    pub(crate) async fn stop_roots_and_wait(&self) {
        {
            let mut closed = self.0.closed.lock().unwrap();
            *closed = true;
            self.0.tasks.close();
            self.0.stop.cancel();
        }
        self.0.tasks.wait().await;
    }

    /// Metadata queries remain available for owner-driven stream cleanup after external roots
    /// stop. Final retirement closes this admission too, before the oplog actor is stopped.
    pub(crate) async fn stop_and_wait(&self) -> Result<(), String> {
        self.stop_roots_and_wait().await;
        {
            let mut closed = self.0.metadata_closed.lock().unwrap();
            *closed = true;
            self.0.metadata.close();
        }
        self.0.metadata.wait().await;
        let owners = {
            let mut actors = self.0.actors.lock().unwrap();
            actors.closed = true;
            actors.owners.retain(|owner| owner.strong_count() != 0);
            actors
                .owners
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for owner in &owners {
            owner.request_stop();
        }
        let mut result = Ok(());
        for owner in owners {
            let completed = owner.stop_and_wait().await;
            result = result.and(completed);
        }
        result
    }

    pub(crate) fn spawn_metadata<F>(&self, future: F) -> Result<JoinHandle<F::Output>, &'static str>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let closed = self.0.metadata_closed.lock().unwrap();
        if *closed {
            return Err("Oplog is stopping");
        }
        let worker = self.pin_worker();
        Ok(self.0.metadata.spawn(async move {
            let _worker = worker;
            future.await
        }))
    }

    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let closed = self.0.closed.lock().unwrap();
        if !*closed {
            let stop = self.0.stop.clone();
            let worker = self.pin_worker();
            self.0.tasks.spawn(async move {
                let _worker = worker;
                tokio::select! {
                    biased;
                    _ = stop.cancelled() => {}
                    _ = future => {}
                }
            });
        }
    }

    /// On ordinary completion this only polls the existing handle. Cancellation transfers
    /// the join, not the work, so storage/cache updates already in progress finish normally.
    pub(crate) fn finish_on_drop<T: Send + 'static>(
        &self,
        task: JoinHandle<T>,
    ) -> FinishOnDrop<'_, T> {
        FinishOnDrop {
            owner: self,
            task: Some(task),
        }
    }

    pub(crate) fn children<T: Send + 'static>(&self) -> JoinedChildren<'_, T> {
        JoinedChildren {
            owner: self,
            tasks: JoinSet::new(),
        }
    }
}

/// A transport can bind after validating its first request and acquiring the actual worker.
/// The scope outlives the root future, including destruction of all captured stream handles.
#[derive(Default)]
pub(crate) struct TaskScope {
    binding: OnceLock<TaskBinding>,
    bound: Notify,
}

struct TaskBinding {
    owner: WorkerTasks,
    _worker: Option<Arc<dyn Send + Sync>>,
    // Last so worker destruction precedes the completion observed by stop_and_wait.
    _token: TaskTrackerToken,
}

impl TaskScope {
    pub(crate) fn bind(&self, owner: &WorkerTasks) -> Result<(), &'static str> {
        if let Some(existing) = self.binding.get() {
            assert!(Arc::ptr_eq(&existing.owner.0, &owner.0));
            return Ok(());
        }
        let closed = owner.0.closed.lock().unwrap();
        if *closed {
            return Err("Worker is being deleted");
        }
        assert!(
            self.binding
                .set(TaskBinding {
                    owner: owner.clone(),
                    _worker: owner.pin_worker(),
                    _token: owner.0.tasks.token(),
                })
                .is_ok()
        );
        self.bound.notify_one();
        Ok(())
    }

    pub(crate) async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let stopped = async {
            loop {
                if let Some(binding) = self.binding.get() {
                    binding.owner.0.stop.cancelled().await;
                    return;
                }
                self.bound.notified().await;
            }
        };
        tokio::select! {
            biased;
            _ = stopped => None,
            result = future => Some(result),
        }
    }
}

pub(crate) struct FinishOnDrop<'a, T: Send + 'static> {
    owner: &'a WorkerTasks,
    task: Option<JoinHandle<T>>,
}

impl<T: Send + 'static> Future for FinishOnDrop<'_, T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = Pin::new(this.task.as_mut().unwrap()).poll(cx);
        if result.is_ready() {
            this.task = None;
        }
        result
    }
}

impl<T: Send + 'static> Drop for FinishOnDrop<'_, T> {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            let worker = self.owner.pin_worker();
            self.owner.0.tasks.spawn(async move {
                let _worker = worker;
                let _ = task.await;
            });
        }
    }
}

pub(crate) struct JoinedChildren<'a, T: Send + 'static> {
    owner: &'a WorkerTasks,
    tasks: JoinSet<T>,
}

impl<T: Send + 'static> Deref for JoinedChildren<'_, T> {
    type Target = JoinSet<T>;

    fn deref(&self) -> &Self::Target {
        &self.tasks
    }
}

impl<T: Send + 'static> DerefMut for JoinedChildren<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tasks
    }
}

impl<T: Send + 'static> Drop for JoinedChildren<'_, T> {
    fn drop(&mut self) {
        if !self.tasks.is_empty() {
            let mut tasks = std::mem::take(&mut self.tasks);
            tasks.abort_all();
            let worker = self.owner.pin_worker();
            self.owner.0.tasks.spawn(async move {
                let _worker = worker;
                while tasks.join_next().await.is_some() {}
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
    async fn join_only_runtime_keeps_retirement_pending_through_panic_cleanup() {
        let owner = WorkerTasks::default();
        let scope = TaskScope::default();
        scope.bind(&owner).unwrap();
        let (release_runtime, runtime_released) = oneshot::channel();
        let (state_dropped, state_dropped_rx) = oneshot::channel();
        let (cleanup_started, cleanup_started_rx) = oneshot::channel();
        let (release_cleanup, cleanup_released) = oneshot::channel();
        let runtime = tokio::spawn(async move {
            let _completion = scope;
            crate::worker::invocation_loop::run_invocation_loop_task(
                async move {
                    let _state = SignalOnDrop(Some(state_dropped));
                    runtime_released.await.unwrap();
                    panic!("injected runtime failure");
                },
                move |_| async move {
                    cleanup_started.send(()).unwrap();
                    cleanup_released.await.unwrap();
                },
            )
            .await;
        });
        let retirement = owner.stop_and_wait();
        tokio::pin!(retirement);
        assert!(futures::poll!(retirement.as_mut()).is_pending());
        assert!(TaskScope::default().bind(&owner).is_err());
        assert!(!runtime.is_finished());
        release_runtime.send(()).unwrap();
        state_dropped_rx.await.unwrap();
        cleanup_started_rx.await.unwrap();
        assert!(futures::poll!(retirement.as_mut()).is_pending());
        release_cleanup.send(()).unwrap();
        retirement.await.unwrap();
        runtime.await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn stop_joins_nested_storage_after_root_and_child_are_cancelled() {
        let shared = WorkerTasks::default();
        let worker = Arc::new(());
        let weak_worker = Arc::downgrade(&worker);
        let old_owner = shared.for_worker(worker.clone());
        let replacement_owner = shared.for_worker(Arc::new(()));
        let writes = Arc::new(AtomicUsize::new(0));
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let (child_dropped, child_dropped_rx) = oneshot::channel();
        let root = tokio::spawn({
            let writes = writes.clone();
            async move {
                let scope = TaskScope::default();
                scope.bind(&old_owner).unwrap();
                scope
                    .run(async {
                        let mut children = old_owner.children();
                        let child_owner = old_owner.clone();
                        children.spawn(async move {
                            let _dropped = SignalOnDrop(Some(child_dropped));
                            let storage = tokio::spawn(async move {
                                started.send(()).unwrap();
                                release_rx.await.unwrap();
                                writes.fetch_add(1, Ordering::SeqCst);
                                Err::<(), _>("storage failed after completing its owned work")
                            });
                            child_owner.finish_on_drop(storage).await.unwrap()
                        });
                        std::future::pending::<()>().await;
                    })
                    .await
            }
        });
        started_rx.await.unwrap();
        drop(worker);
        assert!(
            weak_worker.upgrade().is_some(),
            "the actual worker must remain pinned"
        );
        let stop = replacement_owner.stop_and_wait();
        tokio::pin!(stop);
        assert!(futures::poll!(stop.as_mut()).is_pending());
        assert!(root.await.unwrap().is_none());
        child_dropped_rx.await.unwrap();
        assert!(
            futures::poll!(stop.as_mut()).is_pending(),
            "joining only the root loses its storage child"
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert!(
            weak_worker.upgrade().is_some(),
            "transferred joins retain the old worker, not its replacement"
        );
        release.send(()).unwrap();
        stop.await.unwrap();
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert!(weak_worker.upgrade().is_none());
        assert!(TaskScope::default().bind(&shared).is_err());
        let recreated = WorkerTasks::default();
        assert!(TaskScope::default().bind(&recreated).is_ok());
    }

    #[test]
    #[timeout("30s")]
    async fn aborted_root_transfers_child_join_before_releasing_its_token() {
        let owner = WorkerTasks::default();
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let root = tokio::spawn({
            let owner = owner.clone();
            async move {
                let scope = TaskScope::default();
                scope.bind(&owner).unwrap();
                scope
                    .run(async {
                        let child = tokio::spawn(async move {
                            started.send(()).unwrap();
                            release_rx.await.unwrap();
                        });
                        owner.finish_on_drop(child).await.unwrap();
                    })
                    .await
            }
        });
        started_rx.await.unwrap();
        root.abort();
        assert!(root.await.unwrap_err().is_cancelled());
        let stop = owner.stop_and_wait();
        tokio::pin!(stop);
        assert!(futures::poll!(stop.as_mut()).is_pending());
        release.send(()).unwrap();
        stop.await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn normal_child_wait_has_no_registration_or_owner_clone() {
        let owner = WorkerTasks::default();
        let scope = TaskScope::default();
        scope.bind(&owner).unwrap();
        let count = Arc::strong_count(&owner.0);
        for expected in [17, 3, 29] {
            let child = owner.finish_on_drop(tokio::spawn(async move { expected }));
            assert_eq!(owner.0.tasks.len(), 1);
            assert_eq!(Arc::strong_count(&owner.0), count);
            assert_eq!(child.await.unwrap(), expected);
            assert_eq!(owner.0.tasks.len(), 1);
            assert_eq!(Arc::strong_count(&owner.0), count);
        }
        drop(scope);
        assert!(owner.0.tasks.is_empty());
        owner.stop_and_wait().await.unwrap();
    }

    #[test]
    #[timeout("30s")]
    async fn bind_after_stop_never_polls_work_or_admits_background_tasks() {
        let owner = WorkerTasks::default();
        owner.stop_and_wait().await.unwrap();
        let scope = TaskScope::default();
        assert!(scope.bind(&owner).is_err());
        owner.spawn(async { panic!("closed owner admitted a task") });
        assert!(owner.0.tasks.is_empty());
        owner.stop_and_wait().await.unwrap();
    }

    #[test]
    #[ignore]
    async fn root_admission_throughput() {
        const ROOTS: usize = 20_000;
        let owner = WorkerTasks::default();
        let worker = Arc::new(());
        let owner = owner.for_worker(worker.clone());
        for tracked in [false, true, false, true] {
            let start = std::time::Instant::now();
            let mut roots = JoinSet::new();
            for _ in 0..ROOTS {
                let owner = owner.clone();
                roots.spawn(async move {
                    let work = async {
                        tokio::task::yield_now().await;
                        std::hint::black_box(37)
                    };
                    if tracked {
                        let scope = TaskScope::default();
                        scope.bind(&owner).unwrap();
                        scope.run(work).await.unwrap()
                    } else {
                        work.await
                    }
                });
                if roots.len() >= 128 {
                    assert_eq!(roots.join_next().await.unwrap().unwrap(), 37);
                }
            }
            while let Some(result) = roots.join_next().await {
                assert_eq!(result.unwrap(), 37);
            }
            println!(
                "tracked={tracked}, roots={ROOTS}, elapsed={:?}, roots/s={:.0}",
                start.elapsed(),
                ROOTS as f64 / start.elapsed().as_secs_f64()
            );
        }
        owner.stop_and_wait().await.unwrap();
    }
}
