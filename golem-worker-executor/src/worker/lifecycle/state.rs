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
    Running(&'a RunningWorker),
    Stopping,
    CleanupFailed(&'a WorkerExecutorError),
    Deleting,
}

pub struct UnresolvedPhase<'a> {
    lifecycle: &'a mut WorkerLifecycle,
}

pub struct InitializingPhase<'a> {
    lifecycle: &'a mut WorkerLifecycle,
    completion: Shared<BoxFuture<'static, Result<bool, WorkerExecutorError>>>,
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
        completion: Shared<BoxFuture<'static, Result<bool, WorkerExecutorError>>>,
    ) -> InitializingPhase<'a> {
        self.lifecycle.phase = WorkerInstance::Initializing(completion.clone());
        InitializingPhase {
            lifecycle: self.lifecycle,
            completion,
        }
    }
}

impl InitializingPhase<'_> {
    pub fn completion(&self) -> Shared<BoxFuture<'static, Result<bool, WorkerExecutorError>>> {
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
            WorkerInstance::Running(ref running) => LifecycleTransition::Running(running),
            WorkerInstance::Stopping(_) => LifecycleTransition::Stopping,
            WorkerInstance::CleanupFailed(ref error) => LifecycleTransition::CleanupFailed(error),
            WorkerInstance::Deleting(_) => LifecycleTransition::Deleting,
        }
    }

    pub fn begin_store_attempt(&mut self) -> bool {
        self.phase
            .running_mut()
            .is_some_and(|running| std::mem::take(&mut running.owner_generation_started))
    }

    pub fn owner_generation_started(&mut self, start_attempt: Uuid) {
        if let Some(running) = self.phase.running_mut()
            && running.start_attempt == start_attempt
        {
            running.owner_generation_started = true;
        }
    }

    /// The running worker, also while a deletion retains it.
    pub fn running_runtime(&self) -> Option<&RunningWorker> {
        self.phase.running_runtime()
    }

    /// The cleanup failure that the runtime reports, also while a deletion retains it.
    pub fn cleanup_failure(&self) -> Option<&WorkerExecutorError> {
        self.phase.cleanup_failure()
    }

    /// Whether the runtime, also while a deletion retains it, still waits for a permit, runs or
    /// stops.
    pub fn runtime_active(&self) -> bool {
        self.phase.runtime_active()
    }

    /// The state that the last completed stop of the runtime left, also while a deletion retains
    /// it. `None` while the runtime waits for a permit, runs or stops, and before initialization
    /// completes.
    pub fn stop_outcome(&self) -> Option<FinalWorkerState> {
        self.phase.stop_outcome()
    }

    pub fn complete_deletion_stage(&mut self, stage: WorkerDeletionStage) {
        if let WorkerInstance::Deleting(deleting) = &mut self.phase {
            deleting.completed_stage = deleting.completed_stage.max(stage);
        }
    }

    /// Claims deletion and retains the runtime of the worker. A worker that has not completed
    /// initialization has no runtime to retain, so nothing is claimed.
    pub fn claim_deletion(&mut self) -> Option<(DeleteOutcome, Option<DeletionSender>)> {
        if let WorkerInstance::Deleting(deleting) = &mut self.phase {
            return Some(match deleting.handle.result() {
                None | Some(Ok(())) => (
                    DeleteOutcome::AlreadyDeleting(deleting.handle.clone()),
                    None,
                ),
                Some(Err(_)) => {
                    let (handle, sender) = DeletionHandle::new();
                    deleting.handle = handle.clone();
                    (DeleteOutcome::Started(handle), Some(sender))
                }
            });
        }
        let phase = std::mem::replace(&mut self.phase, WorkerInstance::Unresolved);
        match WorkerRuntime::try_from(phase) {
            Ok(runtime) => {
                let (handle, sender) = DeletionHandle::new();
                self.phase = WorkerInstance::Deleting(DeletingWorker {
                    runtime: DeletionRuntime::Retained(runtime),
                    handle: handle.clone(),
                    completed_stage: WorkerDeletionStage::Claimed,
                });
                Some((DeleteOutcome::Started(handle), Some(sender)))
            }
            Err(phase) => {
                self.phase = phase;
                None
            }
        }
    }

    /// Records that a deletion's cleanup retry joined the physical cleanup of its stopped runtime.
    pub fn deletion_cleanup_completed(&mut self) {
        if let WorkerInstance::Deleting(deleting) = &mut self.phase {
            deleting.runtime = DeletionRuntime::Stopped;
        }
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
        // A cancelled or unwound stop retains cleanup health. Physical ownership stays with
        // the invocation loop or the continuation that already owns its join handle.
        let fallback = match (self.phase.retained_cleanup_failure(), final_state) {
            (Some(error), _) => WorkerRuntime::CleanupFailed(error.clone()),
            (None, FinalWorkerState::CleanupFailed(error))
                if origin == StopOrigin::InvocationLoop =>
            {
                WorkerRuntime::CleanupFailed(error.clone())
            }
            _ => WorkerRuntime::Unloaded {
                startup_failure: None,
            },
        };
        let previous = self.phase.replace_runtime(fallback)?;
        let transition = StopTransition {
            phase: &mut self.phase,
        };
        let previous = match previous {
            WorkerRuntime::Unloaded { .. } => StopPhase::Unloaded(UnloadedStop { transition }),
            WorkerRuntime::CleanupFailed(error) => {
                StopPhase::CleanupFailed(CleanupFailedStop { transition, error })
            }
            WorkerRuntime::WaitingForPermit(waiting) => StopPhase::WaitingForPermit(WaitingStop {
                transition,
                waiting,
            }),
            WorkerRuntime::Running(running) => StopPhase::Running(match origin {
                StopOrigin::InvocationLoop => RunningStop::InvocationLoop(RunningLoopStop {
                    transition,
                    running,
                }),
                StopOrigin::External => RunningStop::External(RunningExternalStop {
                    transition,
                    running,
                }),
            }),
            WorkerRuntime::Stopping(stopping) => StopPhase::Stopping(match origin {
                StopOrigin::InvocationLoop => StoppingStop::InvocationLoop(StoppingLoopStop {
                    transition,
                    stopping,
                }),
                StopOrigin::External => StoppingStop::External(StoppingExternalStop {
                    transition,
                    stopping,
                }),
            }),
        };
        Some(previous)
    }

    pub fn record_loop_failure(&mut self, error: WorkerExecutorError) {
        super::stop::merge_run_loop_failure(&mut self.phase, error);
    }

    pub fn finish_external_stop(&mut self) -> Option<PendingLiveInvocationDisposition> {
        let stopping = self.phase.stopping_mut()?;
        let pending = stopping.pending_live_invocations;
        let final_state = std::mem::replace(
            &mut stopping.final_state,
            FinalWorkerState::Unloaded {
                startup_failure: None,
            },
        );
        self.phase.complete_runtime_stop(final_state);
        Some(pending)
    }
}

/// Reads and replaces the runtime that a phase holds. A deletion retains the runtime of the
/// worker it claimed, so each method looks through `Deleting` to that runtime.
impl WorkerInstance {
    /// Installs `next` in place of the runtime and returns the previous runtime. A phase without
    /// a runtime stays unchanged.
    fn replace_runtime(&mut self, next: WorkerRuntime) -> Option<WorkerRuntime> {
        match self {
            Self::Deleting(deleting) => match &mut deleting.runtime {
                DeletionRuntime::Retained(runtime) => Some(std::mem::replace(runtime, next)),
                DeletionRuntime::Stopped => None,
            },
            phase => match WorkerRuntime::try_from(std::mem::replace(phase, next.into())) {
                Ok(previous) => Some(previous),
                Err(without_runtime) => {
                    *phase = without_runtime;
                    None
                }
            },
        }
    }

    pub(in crate::worker) fn install_runtime(&mut self, runtime: WorkerRuntime) {
        match self {
            Self::Deleting(deleting) => deleting.runtime = DeletionRuntime::Retained(runtime),
            phase => *phase = runtime.into(),
        }
    }

    /// Installs the state that a completed stop leaves. `FinalWorkerState::Deleting` is requested
    /// only by the stop of a claimed deletion, which then retains a stopped runtime.
    fn complete_runtime_stop(&mut self, final_state: FinalWorkerState) {
        let runtime = match final_state {
            FinalWorkerState::Unloaded { startup_failure } => {
                WorkerRuntime::Unloaded { startup_failure }
            }
            FinalWorkerState::CleanupFailed(error) => WorkerRuntime::CleanupFailed(error),
            FinalWorkerState::Deleting => {
                if let Self::Deleting(deleting) = self {
                    deleting.runtime = DeletionRuntime::Stopped;
                    return;
                }
                WorkerRuntime::Unloaded {
                    startup_failure: None,
                }
            }
        };
        self.install_runtime(runtime);
    }

    fn running_mut(&mut self) -> Option<&mut RunningWorker> {
        match self {
            Self::Running(running)
            | Self::Deleting(DeletingWorker {
                runtime: DeletionRuntime::Retained(WorkerRuntime::Running(running)),
                ..
            }) => Some(running),
            _ => None,
        }
    }

    pub(in crate::worker) fn stopping_mut(&mut self) -> Option<&mut StoppingWorker> {
        match self {
            Self::Stopping(stopping)
            | Self::Deleting(DeletingWorker {
                runtime: DeletionRuntime::Retained(WorkerRuntime::Stopping(stopping)),
                ..
            }) => Some(stopping),
            _ => None,
        }
    }

    fn running_runtime(&self) -> Option<&RunningWorker> {
        match self {
            Self::Running(running)
            | Self::Deleting(DeletingWorker {
                runtime: DeletionRuntime::Retained(WorkerRuntime::Running(running)),
                ..
            }) => Some(running),
            _ => None,
        }
    }

    pub(in crate::worker) fn cleanup_failure(&self) -> Option<&WorkerExecutorError> {
        match self {
            Self::CleanupFailed(error)
            | Self::Deleting(DeletingWorker {
                runtime: DeletionRuntime::Retained(WorkerRuntime::CleanupFailed(error)),
                ..
            }) => Some(error),
            _ => None,
        }
    }

    /// A cleanup failure that a stopped runtime reports, or that a stop in progress will leave.
    pub(in crate::worker) fn retained_cleanup_failure(&self) -> Option<&WorkerExecutorError> {
        match self {
            Self::Stopping(StoppingWorker {
                final_state: FinalWorkerState::CleanupFailed(error),
                ..
            })
            | Self::Deleting(DeletingWorker {
                runtime:
                    DeletionRuntime::Retained(WorkerRuntime::Stopping(StoppingWorker {
                        final_state: FinalWorkerState::CleanupFailed(error),
                        ..
                    })),
                ..
            }) => Some(error),
            phase => phase.cleanup_failure(),
        }
    }

    fn runtime_active(&self) -> bool {
        matches!(
            self,
            Self::WaitingForPermit(_)
                | Self::Running(_)
                | Self::Stopping(_)
                | Self::Deleting(DeletingWorker {
                    runtime: DeletionRuntime::Retained(
                        WorkerRuntime::WaitingForPermit(_)
                            | WorkerRuntime::Running(_)
                            | WorkerRuntime::Stopping(_)
                    ),
                    ..
                })
        )
    }

    fn stop_outcome(&self) -> Option<FinalWorkerState> {
        match self {
            Self::Unloaded { startup_failure }
            | Self::Deleting(DeletingWorker {
                runtime: DeletionRuntime::Retained(WorkerRuntime::Unloaded { startup_failure }),
                ..
            }) => Some(FinalWorkerState::Unloaded {
                startup_failure: startup_failure.clone(),
            }),
            Self::Deleting(DeletingWorker {
                runtime: DeletionRuntime::Stopped,
                ..
            }) => Some(FinalWorkerState::Deleting),
            phase => phase
                .cleanup_failure()
                .map(|error| FinalWorkerState::CleanupFailed(error.clone())),
        }
    }
}

impl From<WorkerRuntime> for WorkerInstance {
    fn from(runtime: WorkerRuntime) -> Self {
        match runtime {
            WorkerRuntime::Unloaded { startup_failure } => Self::Unloaded { startup_failure },
            WorkerRuntime::CleanupFailed(error) => Self::CleanupFailed(error),
            WorkerRuntime::WaitingForPermit(waiting) => Self::WaitingForPermit(waiting),
            WorkerRuntime::Running(running) => Self::Running(running),
            WorkerRuntime::Stopping(stopping) => Self::Stopping(stopping),
        }
    }
}

/// Returns the phase unchanged when it holds no runtime: before initialization completes, and
/// while a deletion retains the runtime.
impl TryFrom<WorkerInstance> for WorkerRuntime {
    type Error = WorkerInstance;

    fn try_from(phase: WorkerInstance) -> Result<Self, Self::Error> {
        match phase {
            WorkerInstance::Unloaded { startup_failure } => Ok(Self::Unloaded { startup_failure }),
            WorkerInstance::CleanupFailed(error) => Ok(Self::CleanupFailed(error)),
            WorkerInstance::WaitingForPermit(waiting) => Ok(Self::WaitingForPermit(waiting)),
            WorkerInstance::Running(running) => Ok(Self::Running(running)),
            WorkerInstance::Stopping(stopping) => Ok(Self::Stopping(stopping)),
            phase @ (WorkerInstance::Unresolved
            | WorkerInstance::Initializing(_)
            | WorkerInstance::Deleting(_)) => Err(phase),
        }
    }
}

/// Holds the lifecycle lock's phase while stop effects await. Dropping it before completion
/// leaves the fallback installed by `begin_stop`; completion consumes the handle.
struct StopTransition<'a> {
    phase: &'a mut WorkerInstance,
}

impl StopTransition<'_> {
    /// Installs the final state in the runtime and returns it.
    fn complete(self, final_state: FinalWorkerState) -> FinalWorkerState {
        self.phase.complete_runtime_stop(final_state.clone());
        final_state
    }
}

pub struct UnloadedStop<'a> {
    transition: StopTransition<'a>,
}

impl UnloadedStop<'_> {
    pub fn complete(self, final_state: FinalWorkerState) -> FinalWorkerState {
        self.transition.complete(final_state)
    }
}

pub struct CleanupFailedStop<'a> {
    transition: StopTransition<'a>,
    error: WorkerExecutorError,
}

impl CleanupFailedStop<'_> {
    pub fn retain(self) {
        self.transition
            .phase
            .install_runtime(WorkerRuntime::CleanupFailed(self.error));
    }
}

pub struct WaitingStop<'a> {
    transition: StopTransition<'a>,
    waiting: WaitingWorker,
}

impl WaitingStop<'_> {
    pub fn complete(self, final_state: FinalWorkerState) -> StopCompletion<WaitingWorker> {
        StopCompletion {
            final_state: self.transition.complete(final_state),
            _retained: self.waiting,
        }
    }
}

pub struct RunningLoopStop<'a> {
    transition: StopTransition<'a>,
    running: RunningWorker,
}

impl RunningLoopStop<'_> {
    pub fn complete(self, final_state: FinalWorkerState) -> StopCompletion<RunningWorker> {
        StopCompletion {
            final_state: self.transition.complete(final_state),
            _retained: self.running,
        }
    }
}

/// Retains the extracted task resources through resident waiter resolution after completion.
#[must_use = "retain the completion until resident waiter resolution finishes"]
pub struct StopCompletion<T> {
    final_state: FinalWorkerState,
    _retained: T,
}

impl<T> StopCompletion<T> {
    pub fn final_state(&self) -> &FinalWorkerState {
        &self.final_state
    }
}

pub struct StoppingLoopStop<'a> {
    transition: StopTransition<'a>,
    stopping: StoppingWorker,
}

impl StoppingLoopStop<'_> {
    pub fn complete(
        self,
        final_state: FinalWorkerState,
    ) -> (
        FinalWorkerState,
        OneShotEvent,
        PendingLiveInvocationDisposition,
    ) {
        let pending_live_invocations = self.stopping.pending_live_invocations;
        let (final_state, notify) =
            super::stop::complete_stopping_worker(self.stopping, final_state);
        (
            self.transition.complete(final_state),
            notify,
            pending_live_invocations,
        )
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
        self.transition
            .phase
            .install_runtime(WorkerRuntime::Stopping(self.stopping));
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
        self.transition
            .phase
            .install_runtime(WorkerRuntime::Stopping(StoppingWorker {
                notify: notify.clone(),
                final_state,
                pending_live_invocations,
            }));
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
