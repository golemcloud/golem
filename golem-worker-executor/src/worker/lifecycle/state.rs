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

use crate::worker::*;

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

/// The lifecycle mutex protects this owner. Callers may inspect its phase, but only named
/// transitions can replace it. Running owns an invocation loop, not necessarily a resident Store.
#[derive(Debug)]
pub struct WorkerLifecycle {
    phase: WorkerInstance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopOrigin {
    InvocationLoop,
    External,
}

pub enum StopPhase<'a> {
    Unloaded(UnloadedStop<'a>),
    CleanupFailed(CleanupFailedStop<'a>),
    WaitingForPermit(WaitingStop<'a>),
    Running(RunningStop<'a>),
    Stopping(StoppingStop<'a>),
}

pub enum RunningStop<'a> {
    InvocationLoop(RunningLoopStop<'a>),
    External(RunningExternalStop<'a>),
}

pub enum StoppingStop<'a> {
    InvocationLoop(StoppingLoopStop<'a>),
    External(StoppingExternalStop<'a>),
}

pub enum LifecycleTransition<'a> {
    Unresolved(UnresolvedPhase<'a>),
    Initializing(InitializingPhase<'a>),
    Unloaded(UnloadedPhase<'a>),
    WaitingForPermit(WaitingPhase<'a>),
    Running,
    Stopping,
    CleanupFailed(&'a WorkerExecutorError),
    Deleting,
}

pub struct UnresolvedPhase<'a> {
    lifecycle: &'a mut WorkerLifecycle,
}

pub struct InitializingPhase<'a> {
    lifecycle: &'a mut WorkerLifecycle,
    completion: Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>,
}

pub struct UnloadedPhase<'a> {
    lifecycle: &'a mut WorkerLifecycle,
}

pub struct WaitingPhase<'a> {
    lifecycle: &'a mut WorkerLifecycle,
    start_attempt: Uuid,
}

impl<'a> UnresolvedPhase<'a> {
    pub fn begin_initialization(
        self,
        completion: Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>,
    ) -> InitializingPhase<'a> {
        self.lifecycle.phase = WorkerInstance::Initializing(completion.clone());
        InitializingPhase {
            lifecycle: self.lifecycle,
            completion,
        }
    }
}

impl InitializingPhase<'_> {
    pub fn completion(&self) -> Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>> {
        self.completion.clone()
    }

    pub fn construction_failed(self) {
        self.lifecycle.phase = WorkerInstance::Unresolved;
    }

    pub fn complete_initialization(self, startup_failure: Option<WorkerExecutorError>) {
        self.lifecycle.phase = WorkerInstance::Unloaded { startup_failure };
    }
}

impl UnloadedPhase<'_> {
    pub fn lifecycle(&self) -> &WorkerLifecycle {
        self.lifecycle
    }

    pub fn startup_failed(self, error: WorkerExecutorError) {
        self.lifecycle.phase = WorkerInstance::Unloaded {
            startup_failure: Some(error),
        };
    }

    pub fn wait_for_permit(self, waiting: WaitingWorker) {
        self.lifecycle.phase = WorkerInstance::WaitingForPermit(waiting);
    }
}

impl WaitingPhase<'_> {
    pub fn start_attempt(&self) -> Uuid {
        self.start_attempt
    }

    pub fn startup_failed(self, error: WorkerExecutorError) {
        self.lifecycle.phase = WorkerInstance::Unloaded {
            startup_failure: Some(error),
        };
    }

    pub fn install_running(self, running: RunningWorker) {
        self.lifecycle.phase = WorkerInstance::Running(running);
    }
}

impl WorkerLifecycle {
    pub fn unresolved() -> Self {
        Self {
            phase: WorkerInstance::Unresolved,
        }
    }

    pub fn state(&self) -> &WorkerInstance {
        &self.phase
    }

    pub fn startup_failure(&self) -> Option<&WorkerExecutorError> {
        self.phase.startup_failure()
    }

    pub fn transition(&mut self) -> LifecycleTransition<'_> {
        match self.phase {
            WorkerInstance::Unresolved => {
                LifecycleTransition::Unresolved(UnresolvedPhase { lifecycle: self })
            }
            WorkerInstance::Initializing(ref completion) => {
                let completion = completion.clone();
                LifecycleTransition::Initializing(InitializingPhase {
                    lifecycle: self,
                    completion,
                })
            }
            WorkerInstance::Unloaded { .. } => {
                LifecycleTransition::Unloaded(UnloadedPhase { lifecycle: self })
            }
            WorkerInstance::WaitingForPermit(ref waiting) => {
                let start_attempt = waiting.start_attempt;
                LifecycleTransition::WaitingForPermit(WaitingPhase {
                    lifecycle: self,
                    start_attempt,
                })
            }
            WorkerInstance::Running(_) => LifecycleTransition::Running,
            WorkerInstance::Stopping(_) => LifecycleTransition::Stopping,
            WorkerInstance::CleanupFailed(ref error) => LifecycleTransition::CleanupFailed(error),
            WorkerInstance::Deleting(_) | WorkerInstance::StoppedForDeletion => {
                LifecycleTransition::Deleting
            }
        }
    }

    pub fn begin_store_attempt(&mut self) -> bool {
        match self.runtime_mut() {
            WorkerInstance::Running(running) => {
                std::mem::take(&mut running.owner_generation_started)
            }
            _ => false,
        }
    }

    pub fn owner_generation_started(&mut self, start_attempt: Uuid) {
        if let WorkerInstance::Running(running) = self.runtime_mut()
            && running.start_attempt == start_attempt
        {
            running.owner_generation_started = true;
        }
    }

    pub fn deletion_runtime(&self) -> &WorkerInstance {
        self.phase.deletion_runtime()
    }

    fn runtime_mut(&mut self) -> &mut WorkerInstance {
        match &mut self.phase {
            WorkerInstance::Deleting(deleting) => &mut deleting.runtime,
            phase => phase,
        }
    }

    pub fn complete_deletion_stage(&mut self, stage: WorkerDeletionStage) {
        if let WorkerInstance::Deleting(deleting) = &mut self.phase {
            deleting.completed_stage = deleting.completed_stage.max(stage);
        }
    }

    pub fn claim_deletion(
        &mut self,
    ) -> (
        DeleteOutcome,
        Option<oneshot::Sender<Result<(), WorkerExecutorError>>>,
    ) {
        match &mut self.phase {
            WorkerInstance::Deleting(deleting) => match deleting.handle.result() {
                None | Some(Ok(())) => (
                    DeleteOutcome::AlreadyDeleting(deleting.handle.clone()),
                    None,
                ),
                Some(Err(_)) => {
                    let (handle, sender) = DeletionHandle::new();
                    deleting.handle = handle.clone();
                    (DeleteOutcome::Started(handle), Some(sender))
                }
            },
            _ => {
                let (handle, sender) = DeletionHandle::new();
                let runtime = std::mem::replace(
                    &mut self.phase,
                    WorkerInstance::Unloaded {
                        startup_failure: None,
                    },
                );
                self.phase = WorkerInstance::Deleting(DeletingWorker {
                    runtime: Box::new(runtime),
                    handle: handle.clone(),
                    completed_stage: WorkerDeletionStage::Claimed,
                });
                (DeleteOutcome::Started(handle), Some(sender))
            }
        }
    }

    pub fn deletion_cleanup_completed(&mut self) {
        *self.runtime_mut() = WorkerInstance::StoppedForDeletion;
    }

    pub fn retained_cleanup_failure(&self) -> Option<&WorkerExecutorError> {
        self.phase.retained_cleanup_failure()
    }

    pub fn ensure_not_deleting(&self) -> Result<(), WorkerExecutorError> {
        self.phase.ensure_not_deleting()
    }

    pub fn deletion_owns_retirement(&self) -> bool {
        self.phase.deletion_owns_retirement()
    }

    pub fn begin_stop(
        &mut self,
        origin: StopOrigin,
        final_state: &FinalWorkerState,
    ) -> Option<StopPhase<'_>> {
        let runtime = self.runtime_mut();
        // A cancelled or unwound stop retains cleanup health. Physical ownership stays with
        // the invocation loop or the continuation that already owns its join handle.
        let fallback = match (&*runtime, final_state) {
            (WorkerInstance::CleanupFailed(error), _)
            | (
                WorkerInstance::Stopping(StoppingWorker {
                    final_state: FinalWorkerState::CleanupFailed(error),
                    ..
                }),
                _,
            ) => WorkerInstance::CleanupFailed(error.clone()),
            (_, FinalWorkerState::CleanupFailed(error)) if origin == StopOrigin::InvocationLoop => {
                WorkerInstance::CleanupFailed(error.clone())
            }
            _ => WorkerInstance::Unloaded {
                startup_failure: None,
            },
        };
        let previous = std::mem::replace(runtime, fallback);
        let transition = StopTransition { runtime };
        let previous = match previous {
            WorkerInstance::Unloaded { .. } => StopPhase::Unloaded(UnloadedStop { transition }),
            WorkerInstance::CleanupFailed(error) => {
                StopPhase::CleanupFailed(CleanupFailedStop { transition, error })
            }
            WorkerInstance::WaitingForPermit(waiting) => StopPhase::WaitingForPermit(WaitingStop {
                transition,
                waiting,
            }),
            WorkerInstance::Running(running) => StopPhase::Running(match origin {
                StopOrigin::InvocationLoop => RunningStop::InvocationLoop(RunningLoopStop {
                    transition,
                    running,
                }),
                StopOrigin::External => RunningStop::External(RunningExternalStop {
                    transition,
                    running,
                }),
            }),
            WorkerInstance::Stopping(stopping) => StopPhase::Stopping(match origin {
                StopOrigin::InvocationLoop => StoppingStop::InvocationLoop(StoppingLoopStop {
                    transition,
                    stopping,
                }),
                StopOrigin::External => StoppingStop::External(StoppingExternalStop {
                    transition,
                    stopping,
                }),
            }),
            inactive => {
                *transition.runtime = inactive;
                return None;
            }
        };
        Some(previous)
    }

    pub fn record_loop_failure(&mut self, error: WorkerExecutorError) {
        super::stop::merge_run_loop_failure(self.runtime_mut(), error);
    }

    pub fn finish_external_stop(&mut self) -> Option<PendingLiveInvocationDisposition> {
        let runtime = self.runtime_mut();
        match std::mem::replace(
            runtime,
            WorkerInstance::Unloaded {
                startup_failure: None,
            },
        ) {
            WorkerInstance::Stopping(stopping) => {
                let pending = stopping.pending_live_invocations;
                *runtime = stopping.final_state.into_instance();
                Some(pending)
            }
            previous => {
                *runtime = previous;
                None
            }
        }
    }
}

/// Holds the lifecycle lock's runtime slot while stop effects await. Dropping it before
/// completion leaves the fallback installed by `begin_stop`; completion consumes the handle.
struct StopTransition<'a> {
    runtime: &'a mut WorkerInstance,
}

impl<'a> StopTransition<'a> {
    fn complete(self, final_state: FinalWorkerState) -> &'a WorkerInstance {
        *self.runtime = final_state.into_instance();
        self.runtime
    }
}

pub struct UnloadedStop<'a> {
    transition: StopTransition<'a>,
}

impl<'a> UnloadedStop<'a> {
    pub fn complete(self, final_state: FinalWorkerState) -> &'a WorkerInstance {
        self.transition.complete(final_state)
    }
}

pub struct CleanupFailedStop<'a> {
    transition: StopTransition<'a>,
    error: WorkerExecutorError,
}

impl CleanupFailedStop<'_> {
    pub fn retain(self) {
        *self.transition.runtime = WorkerInstance::CleanupFailed(self.error);
    }
}

pub struct WaitingStop<'a> {
    transition: StopTransition<'a>,
    waiting: WaitingWorker,
}

impl<'a> WaitingStop<'a> {
    pub fn complete(self, final_state: FinalWorkerState) -> StopCompletion<'a, WaitingWorker> {
        StopCompletion {
            phase: self.transition.complete(final_state),
            _retained: self.waiting,
        }
    }
}

pub struct RunningLoopStop<'a> {
    transition: StopTransition<'a>,
    running: RunningWorker,
}

impl<'a> RunningLoopStop<'a> {
    pub fn complete(self, final_state: FinalWorkerState) -> StopCompletion<'a, RunningWorker> {
        StopCompletion {
            phase: self.transition.complete(final_state),
            _retained: self.running,
        }
    }
}

/// Retains the extracted task resources through resident waiter resolution after completion.
#[must_use = "retain the completion until resident waiter resolution finishes"]
pub struct StopCompletion<'a, T> {
    phase: &'a WorkerInstance,
    _retained: T,
}

impl<T> StopCompletion<'_, T> {
    pub fn state(&self) -> &WorkerInstance {
        self.phase
    }
}

pub struct StoppingLoopStop<'a> {
    transition: StopTransition<'a>,
    stopping: StoppingWorker,
}

impl<'a> StoppingLoopStop<'a> {
    pub fn complete(
        self,
        final_state: FinalWorkerState,
    ) -> (
        &'a WorkerInstance,
        OneShotEvent,
        PendingLiveInvocationDisposition,
    ) {
        let pending_live_invocations = self.stopping.pending_live_invocations;
        let (instance, notify) = super::stop::complete_stopping_worker(self.stopping, final_state);
        *self.transition.runtime = instance;
        (self.transition.runtime, notify, pending_live_invocations)
    }
}

pub struct StoppingExternalStop<'a> {
    transition: StopTransition<'a>,
    stopping: StoppingWorker,
}

impl StoppingExternalStop<'_> {
    pub fn merge_request(
        &mut self,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        self.stopping
            .merge_request(final_state, pending_live_invocations);
    }

    pub fn join(self) -> StopResult {
        let notify = self.stopping.notify.clone();
        *self.transition.runtime = WorkerInstance::Stopping(self.stopping);
        StopResult::AlreadyStopping { notify }
    }
}

pub struct RunningExternalStop<'a> {
    transition: StopTransition<'a>,
    running: RunningWorker,
}

impl RunningExternalStop<'_> {
    pub fn begin(
        self,
        unload_request: UnloadRequest,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) -> StopResult {
        let run_loop_handle = self.running.stop(unload_request);
        let notify = OneShotEvent::new();
        crate::metrics::workers::dec_worker_memory_resident();
        *self.transition.runtime = WorkerInstance::Stopping(StoppingWorker {
            notify: notify.clone(),
            final_state,
            pending_live_invocations,
        });
        StopResult::NeedsWaitForLoopExit {
            run_loop_handle,
            notify,
        }
    }
}

impl StoppingWorker {
    pub fn merge_request(
        &mut self,
        final_state: FinalWorkerState,
        pending_live_invocations: PendingLiveInvocationDisposition,
    ) {
        self.final_state = super::stop::merge_final_worker_state(
            std::mem::replace(
                &mut self.final_state,
                FinalWorkerState::Unloaded {
                    startup_failure: None,
                },
            ),
            final_state,
        );
        if pending_live_invocations == PendingLiveInvocationDisposition::Fail {
            self.pending_live_invocations = PendingLiveInvocationDisposition::Fail;
        }
    }
}
