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

use super::*;

const INVOCATION_LOOP_DROP_STACK_SIZE: usize = 8 * 1024 * 1024;
impl Drop for InvocationLoopWatcherLifetime {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl RegisteredOwnerOplog {
    fn fence_executor_shutdown(&self) {
        self.shutdown.fence();
    }
}

impl InvocationLoops {
    pub fn new(shutdown_token: CancellationToken) -> Self {
        let task_shutdown_token = CancellationToken::new();
        let owner_oplogs = Arc::new(Mutex::new(Vec::<RegisteredOwnerOplog>::new()));
        let construction_admission = Arc::new(Mutex::new(true));
        let watcher_stop = CancellationToken::new();
        let watcher_lifetime = Arc::new(InvocationLoopWatcherLifetime(watcher_stop.clone()));
        tokio::spawn({
            let shutdown_token = shutdown_token.clone();
            let task_shutdown_token = task_shutdown_token.clone();
            let owner_oplogs = owner_oplogs.clone();
            let construction_admission = construction_admission.clone();
            async move {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {}
                    _ = watcher_stop.cancelled() => return,
                }
                *construction_admission.lock().unwrap() = false;
                for registration in owner_oplogs.lock().unwrap().iter() {
                    registration.fence_executor_shutdown();
                }
                task_shutdown_token.cancel();
            }
        });
        Self {
            shutdown_token,
            task_shutdown_token,
            tracker: TaskTracker::new(),
            owner_oplogs,
            construction_admission,
            exit_stage: Arc::default(),
            _watcher_lifetime: watcher_lifetime,
        }
    }

    pub(crate) fn register_owner_oplog(&self, oplog: Arc<dyn crate::services::oplog::Oplog>) {
        let mut owner_oplogs = self.owner_oplogs.lock().unwrap();
        if !owner_oplogs
            .iter()
            .filter_map(|registered| registered.oplog.upgrade())
            .any(|registered| Arc::ptr_eq(&registered, &oplog))
        {
            let shutdown = oplog.executor_shutdown_handle();
            if self.shutdown_token.is_cancelled() {
                shutdown.fence();
            }
            owner_oplogs.push(RegisteredOwnerOplog {
                oplog: Arc::downgrade(&oplog),
                label: format!("{oplog:?}"),
                tasks: oplog.task_owner().cloned(),
                shutdown,
            });
        }
    }

    pub(crate) fn spawn_construction(
        &self,
        construction: impl Future<Output = ()> + Send + 'static,
    ) -> bool {
        let mut admission = self.construction_admission.lock().unwrap();
        if !*admission || self.shutdown_token.is_cancelled() {
            *admission = false;
            return false;
        }
        self.tracker.spawn(construction);
        true
    }

    pub(crate) fn spawn(
        &self,
        invocation_loop: impl Future<Output = ()> + Send + 'static,
        on_shutdown: impl FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static,
    ) -> JoinHandle<()> {
        let shutdown_token = self.task_shutdown_token.clone();
        self.tracker.spawn(async move {
            let mut invocation_loop = Box::pin(invocation_loop);
            tokio::select! {
                biased;
                _ = shutdown_token.cancelled() => {
                    let drain = on_shutdown();
                    // Suspended Wasmtime calls form a deeply nested future tree whose destructor
                    // can exhaust Tokio's default worker-thread stack.
                    stacker::grow(INVOCATION_LOOP_DROP_STACK_SIZE, move || drop(invocation_loop));
                    drain.await;
                }
                _ = crate::worker::invocation::with_invocation_stack(&mut invocation_loop) => {}
            }
        })
    }

    /// Runs detached entity work under the executor's shutdown boundary. Dropping a caller's
    /// handle still leaves the entity running normally, while executor shutdown abandons the
    /// transient work without running semantic finalization and joins its destruction before a
    /// replacement executor may open the same owner oplog.
    pub(crate) fn spawn_entity<T>(
        &self,
        entity_task: impl Future<Output = T> + Send + 'static,
    ) -> JoinHandle<Option<T>>
    where
        T: Send + 'static,
    {
        let shutdown_token = self.task_shutdown_token.clone();
        self.tracker.spawn(async move {
            let mut entity_task = Box::pin(entity_task);
            tokio::select! {
                biased;
                _ = shutdown_token.cancelled() => {
                    stacker::grow(INVOCATION_LOOP_DROP_STACK_SIZE, move || drop(entity_task));
                    None
                }
                result = crate::worker::invocation::with_invocation_stack(&mut entity_task) => {
                    Some(result)
                }
            }
        })
    }

    /// Whether the owning executor has been shut down. Once true, no new loop makes progress and
    /// [`Self::wait_for_exit`] resolves as soon as the already running ones have exited.
    pub fn is_shut_down(&self) -> bool {
        self.shutdown_token.is_cancelled()
    }

    pub fn same_executor(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner_oplogs, &other.owner_oplogs)
    }

    /// Resolves once every invocation loop of the executor has exited. Only meaningful after the
    /// executor was shut down; a loop of a live executor runs until its worker is unloaded.
    ///
    /// A loop that is executing guest code without reaching an await point cannot be cancelled
    /// once the executor's epoch ticker stopped, so callers should bound this wait.
    pub async fn wait_for_exit(&self) -> Result<(), String> {
        self.set_exit_stage("worker roots");
        *self.construction_admission.lock().unwrap() = false;
        {
            let owner_oplogs = self.owner_oplogs.lock().unwrap();
            for registration in owner_oplogs.iter() {
                registration.fence_executor_shutdown();
            }
        }
        self.task_shutdown_token.cancel();
        self.tracker.close();
        let early_owner_oplogs = self.owner_oplogs.lock().unwrap().clone();
        futures::future::join_all(
            early_owner_oplogs
                .iter()
                .filter_map(|registration| registration.tasks.as_ref())
                .map(crate::worker::tasks::WorkerTasks::stop_roots_and_wait),
        )
        .await;
        self.set_exit_stage("executor tasks");
        self.tracker.wait().await;
        let owner_oplogs = self.owner_oplogs.lock().unwrap().clone();
        let mut result = Ok(());
        for registration in &owner_oplogs {
            if let Some(tasks) = &registration.tasks {
                self.set_exit_stage(format!("worker tasks of {}", registration.label));
                result = result.and(tasks.stop_and_wait().await);
            }
            self.set_exit_stage(format!("oplog close of {}", registration.label));
            result = result.and(registration.shutdown.close_and_wait().await);
        }
        self.set_exit_stage("exited");
        if result.is_ok() {
            let mut registered = self.owner_oplogs.lock().unwrap();
            registered.retain(|oplog| {
                !owner_oplogs
                    .iter()
                    .any(|drained| Weak::ptr_eq(&oplog.oplog, &drained.oplog))
            });
        }
        result
    }

    fn set_exit_stage(&self, stage: impl Into<String>) {
        *self.exit_stage.lock().unwrap() = stage.into();
    }

    /// Describes what a pending [`Self::wait_for_exit`] still waits for: its current stage, the
    /// executor tasks that are still running, and every owner oplog with unfinished worker tasks.
    pub fn describe_pending_exit(&self) -> String {
        let mut description = format!(
            "stage: {}; executor tasks: {}",
            self.exit_stage.lock().unwrap(),
            self.tracker.len()
        );
        for registration in self.owner_oplogs.lock().unwrap().iter() {
            if let Some(pending) = registration
                .tasks
                .as_ref()
                .and_then(crate::worker::tasks::WorkerTasks::describe_pending)
            {
                description.push_str(&format!("; {}: {pending}", registration.label));
            }
        }
        description
    }
}

impl std::fmt::Debug for InvocationLoops {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvocationLoops")
            .field("shut_down", &self.is_shut_down())
            .finish_non_exhaustive()
    }
}
