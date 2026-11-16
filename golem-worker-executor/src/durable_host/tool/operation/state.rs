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

use super::{
    AcquisitionControl, Arc, BodyAdmissionState, BoxFuture, FutureExt, LaneOwnership, Notify,
    OwnerFailureCleanupOwner, OwnerFailureWinner, OwnerInvocationPermit, OwnerInvocationTicket,
    OwnerToolOperations, SerializableToolOperationTerminal, Shared, ToolBodyAdmissionMetadata,
    ToolOperationLaneMetadata, ToolOperationWinner, ToolOperationWinnerMetadata,
    WorkerExecutorError, watch,
};

pub(super) type CleanupTask = Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>;

pub(super) enum OwnerPhase {
    Open,
    Unloading,
    Failed {
        winner: OwnerFailureWinner,
        cleanup: FailureCleanup,
    },
}

pub(super) enum FailureCleanup {
    Unassigned,
    AwaitingClaim(OwnerFailureCleanupOwner),
    Claimed(Arc<CleanupClaim>),
    Running {
        claim: Arc<CleanupClaim>,
        task: CleanupTask,
    },
    SettledSuccessfully(CleanupTask),
    SettledWithError(CleanupTask),
    Aborted {
        owner: OwnerFailureCleanupOwner,
        task: CleanupTask,
    },
}

pub(super) struct CleanupClaim {
    owner: OwnerFailureCleanupOwner,
    winner: OwnerFailureWinner,
}

/// Exclusive authority to start cleanup, not a guarantee that cleanup runs.
/// Dropping this token leaves cleanup Claimed without starting a task or retaining a failed join.
/// An owner with no operations or tracked failure continuations can still begin a new generation.
#[must_use = "call start to run owner failure cleanup; discarding the token does not run cleanup"]
pub(crate) struct OwnerFailureCleanupToken {
    owner: Arc<OwnerToolOperations>,
    claim: Arc<CleanupClaim>,
}

struct RunningCleanup {
    owner: Arc<OwnerToolOperations>,
    claim: Arc<CleanupClaim>,
}

enum CleanupOutcome {
    Settled(Result<(), WorkerExecutorError>),
    Aborted(WorkerExecutorError),
}

struct ClaimedCleanup<'a> {
    cleanup: &'a mut FailureCleanup,
    claim: Arc<CleanupClaim>,
}

impl ClaimedCleanup<'_> {
    fn start(self, task: CleanupTask) {
        *self.cleanup = FailureCleanup::Running {
            claim: self.claim,
            task,
        };
    }
}

impl OwnerFailureCleanupToken {
    pub(crate) fn winner(&self) -> &OwnerFailureWinner {
        &self.claim.winner
    }

    /// The claim is consumed by the task; shared joins only observe its retained result.
    pub(crate) fn start(
        self,
        cleanup: impl Future<Output = Result<(), WorkerExecutorError>> + Send + 'static,
        wake: impl FnOnce() + Send + 'static,
    ) -> CleanupTask {
        let mut state = self.owner.state.lock().unwrap();
        let claimed = match state.phase.claimed_cleanup(&self.claim) {
            Ok(claimed) => claimed,
            Err(error) => return futures::future::ready(Err(error)).boxed().shared(),
        };
        let running = RunningCleanup {
            owner: self.owner.clone(),
            claim: self.claim,
        };
        let task = tokio::spawn(async move {
            let outcome = match std::panic::AssertUnwindSafe(cleanup).catch_unwind().await {
                Ok(result) => {
                    running.owner.wait_owner_settled().await;
                    CleanupOutcome::Settled(result)
                }
                Err(_) => CleanupOutcome::Aborted(WorkerExecutorError::runtime(
                    "tool failure cleanup panicked",
                )),
            };
            match running.finish(outcome)? {
                CleanupOutcome::Settled(result) => {
                    wake();
                    result
                }
                CleanupOutcome::Aborted(error) => Err(error),
            }
        });
        let task = async move {
            task.await.map_err(|error| {
                WorkerExecutorError::runtime(format!("tool failure cleanup task failed: {error}"))
            })?
        }
        .boxed()
        .shared();
        claimed.start(task.clone());
        task
    }
}

impl RunningCleanup {
    fn finish(self, outcome: CleanupOutcome) -> Result<CleanupOutcome, WorkerExecutorError> {
        let mut state = self.owner.state.lock().unwrap();
        let OwnerPhase::Failed { cleanup, .. } = &mut state.phase else {
            return Err(WorkerExecutorError::runtime(
                "cleanup owner failure is no longer selected",
            ));
        };
        let task = match cleanup {
            FailureCleanup::Running { claim, task } if Arc::ptr_eq(claim, &self.claim) => {
                task.clone()
            }
            _ => {
                return Err(WorkerExecutorError::runtime(
                    "cleanup claim is no longer running",
                ));
            }
        };
        *cleanup = match &outcome {
            CleanupOutcome::Settled(Ok(())) => FailureCleanup::SettledSuccessfully(task),
            CleanupOutcome::Settled(Err(_)) => FailureCleanup::SettledWithError(task),
            CleanupOutcome::Aborted(_) => FailureCleanup::Aborted {
                owner: self.claim.owner,
                task,
            },
        };
        drop(state);
        self.owner.changed.notify_waiters();
        Ok(outcome)
    }
}

impl OwnerPhase {
    pub(super) fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }

    pub(super) fn winner(&self) -> Option<&OwnerFailureWinner> {
        match self {
            Self::Failed { winner, .. } => Some(winner),
            Self::Open | Self::Unloading => None,
        }
    }

    pub(super) fn fence_for_unload(&mut self) -> bool {
        if !self.is_open() {
            return false;
        }
        *self = Self::Unloading;
        true
    }

    pub(super) fn select_failure(
        &mut self,
        winner: OwnerFailureWinner,
        cleanup_owner: Option<OwnerFailureCleanupOwner>,
    ) -> bool {
        if self.winner().is_some() {
            return false;
        }
        *self = Self::Failed {
            winner,
            cleanup: cleanup_owner
                .map_or(FailureCleanup::Unassigned, FailureCleanup::AwaitingClaim),
        };
        true
    }

    pub(super) fn claim_cleanup(
        &mut self,
        owner: OwnerFailureCleanupOwner,
        operations: Arc<OwnerToolOperations>,
    ) -> Option<OwnerFailureCleanupToken> {
        let Self::Failed { winner, cleanup } = self else {
            return None;
        };
        if !matches!(cleanup, FailureCleanup::AwaitingClaim(expected) if *expected == owner) {
            return None;
        }
        let claim = Arc::new(CleanupClaim {
            owner,
            winner: winner.clone(),
        });
        *cleanup = FailureCleanup::Claimed(claim.clone());
        Some(OwnerFailureCleanupToken {
            owner: operations,
            claim,
        })
    }

    fn claimed_cleanup(
        &mut self,
        claim: &Arc<CleanupClaim>,
    ) -> Result<ClaimedCleanup<'_>, WorkerExecutorError> {
        let Self::Failed { cleanup, .. } = self else {
            return Err(WorkerExecutorError::runtime(
                "cleanup owner failure is no longer selected",
            ));
        };
        if !matches!(cleanup, FailureCleanup::Claimed(expected) if Arc::ptr_eq(expected, claim)) {
            return Err(WorkerExecutorError::runtime(
                "cleanup claim is no longer available",
            ));
        }
        Ok(ClaimedCleanup {
            cleanup,
            claim: claim.clone(),
        })
    }

    pub(super) fn cleanup_task(&self) -> Option<&CleanupTask> {
        match self {
            Self::Failed {
                cleanup:
                    FailureCleanup::Running { task, .. }
                    | FailureCleanup::SettledSuccessfully(task)
                    | FailureCleanup::SettledWithError(task)
                    | FailureCleanup::Aborted { task, .. },
                ..
            } => Some(task),
            _ => None,
        }
    }

    pub(super) fn interruptible_failure(&self) -> Option<&OwnerFailureWinner> {
        let Self::Failed { winner, cleanup } = self else {
            return None;
        };
        match cleanup {
            FailureCleanup::SettledSuccessfully(_) | FailureCleanup::SettledWithError(_) => {
                Some(winner)
            }
            FailureCleanup::Running { claim, .. }
                if claim.owner == OwnerFailureCleanupOwner::Primary =>
            {
                Some(winner)
            }
            FailureCleanup::Aborted {
                owner: OwnerFailureCleanupOwner::Primary,
                ..
            } => Some(winner),
            _ => None,
        }
    }
}

/// Body progress, result selection and lane ownership have independent lifetimes. All changes
/// run under the owner mutex; terminal publication does not release a lane held for parent cleanup.
pub(super) struct OperationState {
    winner: ToolOperationWinner,
    winner_tx: watch::Sender<ToolOperationWinner>,
    admission: BodyAdmissionState,
    lane: LaneOwnership,
    acquisition_error: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum TerminalResolution {
    Committed,
    RolledBack,
}

pub(super) enum TerminalSelection {
    Cancelled,
    Ordinary(Arc<SerializableToolOperationTerminal>),
}

pub(super) struct OpenTerminal<'a> {
    state: &'a mut OperationState,
}

impl OpenTerminal<'_> {
    pub(super) fn reserve_ordinary(self) {
        self.state.publish(ToolOperationWinner::SelectingOrdinary);
    }
}

pub(super) struct ReadyBody<'a> {
    state: &'a mut OperationState,
}

impl ReadyBody<'_> {
    pub(super) fn queue(self, ticket: OwnerInvocationTicket) {
        self.state.lane = LaneOwnership::Ticket(ticket);
        self.state.admission = BodyAdmissionState::Registered;
    }
}

pub(super) struct StartingAcquisition<'a> {
    state: &'a mut OperationState,
}

impl StartingAcquisition<'_> {
    pub(super) fn install(self, control: AcquisitionControl) {
        self.state.lane = LaneOwnership::Acquiring(control);
    }
}

pub(super) struct AcquiringBody<'a> {
    state: &'a mut OperationState,
}

impl AcquiringBody<'_> {
    pub(super) fn install_grant(self, permit: OwnerInvocationPermit) {
        self.state.lane = LaneOwnership::Permit { _permit: permit };
        self.state.admission = BodyAdmissionState::Running;
    }
}

impl OperationState {
    pub(super) fn new(winner_tx: watch::Sender<ToolOperationWinner>) -> Self {
        Self {
            winner: ToolOperationWinner::Open,
            winner_tx,
            admission: BodyAdmissionState::Staging,
            lane: LaneOwnership::None,
            acquisition_error: None,
        }
    }

    pub(super) fn winner(&self) -> &ToolOperationWinner {
        &self.winner
    }
    pub(super) fn admission(&self) -> BodyAdmissionState {
        self.admission
    }
    pub(super) fn lane(&self) -> &LaneOwnership {
        &self.lane
    }

    fn publish(&mut self, winner: ToolOperationWinner) {
        self.winner = winner.clone();
        self.winner_tx.send_replace(winner);
    }

    pub(super) fn notify_observers(&self) {
        self.winner_tx.send_replace(self.winner.clone());
    }

    pub(super) fn begin_cancel(&mut self) -> bool {
        match self.winner {
            ToolOperationWinner::Open => self.publish(ToolOperationWinner::SelectingCancelled),
            ToolOperationWinner::SelectingCancelled => {}
            _ => return false,
        }
        true
    }

    pub(super) fn begin_ordinary(&mut self) -> bool {
        let Some(open) = self.open_terminal() else {
            return false;
        };
        open.reserve_ordinary();
        true
    }

    pub(super) fn open_terminal(&mut self) -> Option<OpenTerminal<'_>> {
        matches!(self.winner, ToolOperationWinner::Open).then_some(OpenTerminal { state: self })
    }

    pub(super) fn withdraw_uncommitted_cancellation(&mut self, changed: &Notify) {
        if matches!(self.winner, ToolOperationWinner::SelectingCancelled) {
            self.publish(ToolOperationWinner::Open);
            changed.notify_waiters();
        }
    }

    pub(super) fn fence_by_owner(&mut self, trapped: bool) -> bool {
        if !matches!(self.winner, ToolOperationWinner::Open) {
            return false;
        }
        self.publish(if trapped {
            ToolOperationWinner::Trap
        } else {
            ToolOperationWinner::FencedByOwner
        });
        true
    }

    pub(super) fn resolve_selection(
        &mut self,
        selection: TerminalSelection,
        resolution: TerminalResolution,
    ) -> Result<(), WorkerExecutorError> {
        if !matches!(
            (&selection, &self.winner),
            (
                TerminalSelection::Cancelled,
                ToolOperationWinner::SelectingCancelled
            ) | (
                TerminalSelection::Ordinary(_),
                ToolOperationWinner::SelectingOrdinary
            )
        ) {
            return Err(WorkerExecutorError::runtime(
                "owner tool operation selection resolved from the wrong state",
            ));
        }
        self.publish(match (selection, resolution) {
            (_, TerminalResolution::RolledBack) => ToolOperationWinner::Open,
            (TerminalSelection::Cancelled, TerminalResolution::Committed) => {
                ToolOperationWinner::Cancelled
            }
            (TerminalSelection::Ordinary(terminal), TerminalResolution::Committed) => {
                ToolOperationWinner::Ordinary {
                    _terminal: terminal,
                }
            }
        });
        Ok(())
    }

    pub(super) fn mark_body_ready(&mut self) -> bool {
        self.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
    }

    pub(super) fn start_body_without_lane(&mut self) -> bool {
        self.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Running)
    }

    pub(super) fn settle_without_body(&mut self, expected: BodyAdmissionState) -> bool {
        if !matches!(
            expected,
            BodyAdmissionState::Staging | BodyAdmissionState::Ready
        ) {
            return false;
        }
        self.transition_admission(expected, BodyAdmissionState::SettledWithoutBody)
    }

    fn transition_admission(
        &mut self,
        expected: BodyAdmissionState,
        next: BodyAdmissionState,
    ) -> bool {
        if self.admission != expected || self.winner.is_terminal() {
            return false;
        }
        self.admission = next;
        true
    }

    pub(super) fn ready_body(&mut self) -> Option<ReadyBody<'_>> {
        if self.admission == BodyAdmissionState::Ready
            && matches!(self.winner, ToolOperationWinner::Open)
            && matches!(self.lane, LaneOwnership::None)
        {
            Some(ReadyBody { state: self })
        } else {
            None
        }
    }

    pub(super) fn take_ticket(
        &mut self,
    ) -> Result<(OwnerInvocationTicket, StartingAcquisition<'_>), WorkerExecutorError> {
        if self.admission != BodyAdmissionState::Registered {
            return Err(WorkerExecutorError::runtime("tool body is not registered"));
        }
        match std::mem::replace(&mut self.lane, LaneOwnership::None) {
            LaneOwnership::Ticket(ticket) => Ok((ticket, StartingAcquisition { state: self })),
            lane => {
                self.lane = lane;
                Err(WorkerExecutorError::runtime(
                    "registered tool operation lost its lane ticket",
                ))
            }
        }
    }

    pub(super) fn acquiring_body(&mut self) -> Option<AcquiringBody<'_>> {
        if self.admission == BodyAdmissionState::Registered
            && matches!(self.lane, LaneOwnership::Acquiring(_))
            && matches!(self.winner, ToolOperationWinner::Open)
        {
            Some(AcquiringBody { state: self })
        } else {
            None
        }
    }

    pub(super) fn fail_acquisition(&mut self, error: String) {
        if matches!(self.lane, LaneOwnership::Acquiring(_)) {
            self.lane = LaneOwnership::None;
            self.acquisition_error = Some(error);
        }
    }

    pub(super) fn take_acquisition_error(&mut self) -> Option<String> {
        self.acquisition_error.take()
    }

    pub(super) fn take_lane_for_drain(&mut self) -> LaneOwnership {
        std::mem::replace(&mut self.lane, LaneOwnership::None)
    }

    pub(super) fn metadata(
        &self,
    ) -> (
        ToolBodyAdmissionMetadata,
        ToolOperationLaneMetadata,
        ToolOperationWinnerMetadata,
    ) {
        let admission = match self.admission {
            BodyAdmissionState::Staging => ToolBodyAdmissionMetadata::Staging,
            BodyAdmissionState::Ready => ToolBodyAdmissionMetadata::Ready,
            BodyAdmissionState::Registered => ToolBodyAdmissionMetadata::Registered,
            BodyAdmissionState::Running => ToolBodyAdmissionMetadata::Running,
            BodyAdmissionState::SettledWithoutBody => ToolBodyAdmissionMetadata::SettledWithoutBody,
        };
        let lane = match self.lane {
            LaneOwnership::None => ToolOperationLaneMetadata::None,
            LaneOwnership::Ticket(_) => ToolOperationLaneMetadata::Queued,
            LaneOwnership::Acquiring(_) => ToolOperationLaneMetadata::Acquiring,
            LaneOwnership::Permit { .. } => ToolOperationLaneMetadata::Granted,
        };
        let winner = match self.winner {
            ToolOperationWinner::Open => ToolOperationWinnerMetadata::Open,
            ToolOperationWinner::SelectingCancelled => {
                ToolOperationWinnerMetadata::SelectingCancelled
            }
            ToolOperationWinner::SelectingOrdinary => {
                ToolOperationWinnerMetadata::SelectingOrdinary
            }
            ToolOperationWinner::Cancelled => ToolOperationWinnerMetadata::Cancelled,
            ToolOperationWinner::Ordinary { .. } => ToolOperationWinnerMetadata::Ordinary,
            ToolOperationWinner::Trap => ToolOperationWinnerMetadata::Trap,
            ToolOperationWinner::FencedByOwner => ToolOperationWinnerMetadata::FencedByOwner,
        };
        (admission, lane, winner)
    }

    #[cfg(test)]
    pub(super) fn acquiring_for_test(
        winner_tx: watch::Sender<ToolOperationWinner>,
        control: AcquisitionControl,
    ) -> Self {
        Self {
            admission: BodyAdmissionState::Registered,
            lane: LaneOwnership::Acquiring(control),
            ..Self::new(winner_tx)
        }
    }
}
