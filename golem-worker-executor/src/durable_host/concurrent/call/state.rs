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

//! Owning handles for one durable call's lifecycle. Handle transitions consume `self`:
//!
//! ```text
//! ReplayCall
//!     |
//!     | begin_resolution() (also returns the replay claim)
//!     v
//! ResolvingReplay
//!     +-- accept_terminal() ------------------------> ReleasedCall
//!     +-- park_undelivered() -----------------------> ParkedReplay
//!     |
//!     | begin_repair(permit)
//!     v
//! RepairPendingCall
//!     +-- fail() ----------------------------------> destroyed (permit released)
//!     |
//!     | finish_readiness()
//!     v
//! ExecutingCall (also constructed directly for live or unrecorded execution)
//!     +-- enter_direct_terminal() ------------------> ReleasedCall
//!     +-- handoff_terminal_guard() -----------------> ReleasedCall
//!
//! ReplayCall -- begin_cancellation() ---------------> ReleasedCall (+ replay claim)
//! ReplayCall -- session.discard_after_guest_drop() --> ParkedReplay
//! ```
//!
//! The runtime-dispatched session checks the phase before extracting a handle, leaving an
//! empty Released placeholder. The extracted handle owns cleanup across awaits; execution
//! and released handles can be reinserted with `into_session()`. Session abandonment or
//! failure releases cleanup from any resident phase without writing a terminal.
//!
//! Pending repair retains replay-side Drop behavior and owns a permit while readiness is
//! awaited. Readiness enables session-local execution; Store liveness still gates effects.
//! Released retains resources but has no unfinished-call cleanup obligation. It does not
//! imply that an End exists. The terminal guard and delivery token have separate lifecycles.

use super::*;

pub enum CallRecording {
    Recorded { request_upload: PendingUpload },
    Unrecorded,
}

enum Phase<P: DropPolicy> {
    Executing(ExecutingCall<P>),
    Replaying(ReplayCall<P>),
    Parked(ParkedReplay<P>),
    Released(ReleasedCall<P>),
}

/// The dynamic boundary used by ordinary host callers. Once extracted, a phase owns its
/// cleanup obligation and can only transition by consuming its handle.
pub struct SessionState<P: DropPolicy>(Phase<P>);

pub struct ExecutingCall<P: DropPolicy> {
    recording: CallRecording,
    guard: CallGuard<P>,
}

pub struct ReplayCall<P: DropPolicy> {
    claim: ReplayCallHandle,
    guard: CallGuard<P>,
}

pub struct ResolvingReplay<P: DropPolicy> {
    guard: CallGuard<P>,
}

pub struct RepairPendingCall<P: DropPolicy> {
    guard: CallGuard<P>,
}

pub struct ParkedReplay<P: DropPolicy> {
    guard: CallGuard<P>,
}

/// Retains resources after releasing unfinished-call cleanup, including while terminal
/// persistence is still pending. It does not assert that an End has been appended.
pub struct ReleasedCall<P: DropPolicy>(Retained<P>);

enum Retained<P: DropPolicy> {
    Executing(ExecutingCall<P>),
    Replaying(ReplayCall<P>),
    Resolving(ResolvingReplay<P>),
    Parked(ParkedReplay<P>),
    Empty,
}

enum Cleanup {
    Recorded,
    Unrecorded,
    Replay { expected: bool },
    Released,
}

/// Metadata is captured when a phase leaves the session, or just before the session drops.
/// The guard retains its permit until destruction or transfer to a deferred drop event.
struct CallGuard<P: DropPolicy> {
    cleanup: Cleanup,
    permit: Option<LiveCallPermit>,
    context: Option<CallDropContext>,
    _policy: PhantomData<P>,
}

struct CallDropContext {
    call: DroppedCall,
    sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    teardown: TeardownProbe,
    tool_trap_disposal: Arc<AtomicBool>,
}

impl<P: DropPolicy> CallGuard<P> {
    fn new(cleanup: Cleanup, permit: Option<LiveCallPermit>) -> Self {
        Self {
            cleanup,
            permit,
            context: None,
            _policy: PhantomData,
        }
    }

    fn release(&mut self) {
        self.cleanup = Cleanup::Released;
    }
}

impl<P: DropPolicy> Drop for CallGuard<P> {
    fn drop(&mut self) {
        let Some(mut context) = self.context.take() else {
            return;
        };
        if matches!(self.cleanup, Cleanup::Released) {
            return;
        }
        let cancel_on_tool_trap = matches!(self.cleanup, Cleanup::Recorded)
            && (P::CANCEL_ON_TOOL_TRAP || context.call.entity_admission)
            && context.tool_trap_disposal.load(Ordering::Acquire);
        if !cancel_on_tool_trap && context.teardown.is_tearing_down() {
            tracing::debug!(
                start_idx = %context.call.start_idx,
                "durable call abandoned during executor shutdown"
            );
            return;
        }
        match self.cleanup {
            Cleanup::Recorded => {
                if let Some(span) = &mut context.call.span_finished {
                    span.finished_at = Timestamp::now_utc();
                }
                context.call.live_call_permit = self.permit.take();
                if cancel_on_tool_trap {
                    Cancellable::unfinished_drop(context.call, context.cleanup_sink.as_ref());
                } else {
                    P::unfinished_drop(context.call, context.sink.as_ref());
                }
            }
            Cleanup::Replay { expected: true } => tracing::debug!(
                start_idx = %context.call.start_idx,
                "parked undelivered durable call replay handle dropped by the guest"
            ),
            Cleanup::Replay { expected: false } => tracing::warn!(
                start_idx = %context.call.start_idx,
                "replay durable call handle dropped without finishing"
            ),
            Cleanup::Unrecorded | Cleanup::Released => {}
        }
    }
}

impl<P: DropPolicy> ExecutingCall<P> {
    pub fn enter_direct_terminal(mut self) -> ReleasedCall<P> {
        self.guard.release();
        ReleasedCall(Retained::Executing(self))
    }

    pub fn handoff_terminal_guard(self) -> ReleasedCall<P> {
        self.enter_direct_terminal()
    }

    pub fn into_session(self) -> SessionState<P> {
        SessionState(Phase::Executing(self))
    }
}

impl<P: DropPolicy> ReplayCall<P> {
    pub fn begin_resolution(self) -> (ReplayCallHandle, ResolvingReplay<P>) {
        (self.claim, ResolvingReplay { guard: self.guard })
    }

    pub fn begin_cancellation(self) -> (ReplayCallHandle, ReleasedCall<P>) {
        let (claim, resolving) = self.begin_resolution();
        (claim, resolving.accept_terminal())
    }

    fn park(self) -> ParkedReplay<P> {
        let (_, resolving) = self.begin_resolution();
        resolving.park_undelivered()
    }
}

impl<P: DropPolicy> ResolvingReplay<P> {
    pub fn accept_terminal(mut self) -> ReleasedCall<P> {
        self.guard.release();
        ReleasedCall(Retained::Resolving(self))
    }

    pub fn park_undelivered(mut self) -> ParkedReplay<P> {
        self.guard.cleanup = Cleanup::Replay { expected: true };
        ParkedReplay { guard: self.guard }
    }

    pub fn begin_repair(mut self, permit: LiveCallPermit) -> RepairPendingCall<P> {
        self.guard.permit = Some(permit);
        RepairPendingCall { guard: self.guard }
    }
}

impl<P: DropPolicy> RepairPendingCall<P> {
    /// Readiness changes only session-local execution. Store publication still gates effects.
    pub fn finish_readiness(mut self) -> ExecutingCall<P> {
        self.guard.cleanup = Cleanup::Recorded;
        ExecutingCall {
            recording: CallRecording::Recorded {
                request_upload: PendingUpload::already_durable(),
            },
            guard: self.guard,
        }
    }

    pub fn fail(mut self) {
        self.guard.release();
        // Destruction releases the preparation permit immediately.
    }
}

impl<P: DropPolicy> ReleasedCall<P> {
    pub fn into_session(self) -> SessionState<P> {
        SessionState(Phase::Released(self))
    }

    fn guard_mut(&mut self) -> Option<&mut CallGuard<P>> {
        match &mut self.0 {
            Retained::Executing(call) => Some(&mut call.guard),
            Retained::Replaying(call) => Some(&mut call.guard),
            Retained::Resolving(call) => Some(&mut call.guard),
            Retained::Parked(call) => Some(&mut call.guard),
            Retained::Empty => None,
        }
    }
}

impl<P: DropPolicy> SessionState<P> {
    pub fn executing(recording: CallRecording, permit: LiveCallPermit) -> Self {
        let cleanup = match &recording {
            CallRecording::Recorded { .. } => Cleanup::Recorded,
            CallRecording::Unrecorded => Cleanup::Unrecorded,
        };
        ExecutingCall {
            recording,
            guard: CallGuard::new(cleanup, Some(permit)),
        }
        .into_session()
    }

    pub fn replaying(claim: ReplayCallHandle) -> Self {
        Self(Phase::Replaying(ReplayCall {
            claim,
            guard: CallGuard::new(Cleanup::Replay { expected: false }, None),
        }))
    }

    fn execution(&self) -> Option<&ExecutingCall<P>> {
        match &self.0 {
            Phase::Executing(call) | Phase::Released(ReleasedCall(Retained::Executing(call))) => {
                Some(call)
            }
            _ => None,
        }
    }

    pub fn is_executing(&self) -> bool {
        self.execution().is_some()
    }

    pub fn has_recorded_start(&self) -> bool {
        !matches!(
            self.execution(),
            Some(ExecutingCall {
                recording: CallRecording::Unrecorded,
                ..
            })
        )
    }

    pub fn is_recorded_execution(&self) -> bool {
        matches!(
            self.execution(),
            Some(ExecutingCall {
                recording: CallRecording::Recorded { .. },
                ..
            })
        )
    }

    pub fn is_released(&self) -> bool {
        matches!(self.0, Phase::Released(_))
    }

    pub fn request_upload(&self) -> PendingUpload {
        match self.execution() {
            Some(ExecutingCall {
                recording: CallRecording::Recorded { request_upload },
                ..
            }) => request_upload.clone(),
            _ => PendingUpload::already_durable(),
        }
    }

    pub fn clone_permit(&self) -> Option<LiveCallPermit> {
        self.execution().and_then(|call| call.guard.permit.clone())
    }

    pub fn take_historical_reconstruction(&mut self) -> Option<HistoricalReconstruction> {
        match &mut self.0 {
            Phase::Replaying(call) => call.claim.take_historical_reconstruction(),
            _ => None,
        }
    }

    fn take(&mut self) -> Phase<P> {
        std::mem::replace(&mut self.0, Phase::Released(ReleasedCall(Retained::Empty)))
    }

    fn guard_mut(&mut self) -> Option<&mut CallGuard<P>> {
        match &mut self.0 {
            Phase::Executing(call) => Some(&mut call.guard),
            Phase::Replaying(call) => Some(&mut call.guard),
            Phase::Parked(call) => Some(&mut call.guard),
            Phase::Released(call) => call.guard_mut(),
        }
    }

    pub fn abandon_for_trap(&mut self) {
        let retained = match self.take() {
            Phase::Executing(call) => Retained::Executing(call),
            Phase::Replaying(call) => Retained::Replaying(call),
            Phase::Parked(call) => Retained::Parked(call),
            Phase::Released(call) => call.0,
        };
        let mut released = ReleasedCall(retained);
        if let Some(guard) = released.guard_mut() {
            guard.release();
        }
        self.0 = Phase::Released(released);
    }

    pub fn fail(&mut self) {
        self.abandon_for_trap();
    }

    pub fn discard_after_guest_drop(&mut self) {
        self.0 = match self.take() {
            Phase::Replaying(call) => Phase::Parked(call.park()),
            phase => phase,
        };
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    fn drop_context(&self) -> CallDropContext {
        CallDropContext {
            call: self.dropped_call_snapshot(None),
            sink: self.drop_sink.clone(),
            cleanup_sink: self.cleanup_sink.clone(),
            teardown: self.teardown_probe(),
            tool_trap_disposal: self.tool_trap_disposal.clone(),
        }
    }

    pub fn take_executing_call(&mut self) -> Result<ExecutingCall<P>, WorkerExecutorError> {
        let context = self.drop_context();
        match self.state.take() {
            Phase::Executing(mut call) => {
                call.guard.context = Some(context);
                Ok(call)
            }
            phase => {
                self.state.0 = phase;
                Err(self.phase_error("executing"))
            }
        }
    }

    pub fn take_replay_call(&mut self) -> Result<ReplayCall<P>, WorkerExecutorError> {
        let context = self.drop_context();
        match self.state.take() {
            Phase::Replaying(mut call) => {
                call.guard.context = Some(context);
                Ok(call)
            }
            phase => {
                self.state.0 = phase;
                Err(self.phase_error("claimed replay"))
            }
        }
    }

    fn phase_error(&self, expected: &str) -> WorkerExecutorError {
        let actual = match self.state.0 {
            Phase::Executing(_) => "executing",
            Phase::Replaying(_) => "claimed replay",
            Phase::Parked(_) => "parked replay",
            Phase::Released(_) => "released",
        };
        WorkerExecutorError::runtime(format!(
            "{} at Start {} requires {expected}, but the session is {actual}",
            Pair::FQFN,
            self.start_idx
        ))
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> Drop for DurableCallSession<Pair, P> {
    fn drop(&mut self) {
        let context = self.drop_context();
        if let Some(guard) = self.state.guard_mut() {
            guard.context = Some(context);
        }
    }
}
