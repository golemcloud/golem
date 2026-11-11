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

//! Generation admission, independent of outstanding call and node leases.
//!
//! [`transition`] defines every state change. The registry applies each event under its state
//! lock. Replay admits calls just like ordinary open admission. Sealing and invalidation stop
//! new calls, but existing calls and internal cleanup retain their leases until completion.

use super::{AccessError, GenerationRegistry};
use std::sync::Arc;

/// Which new calls and reconstruction handles a generation accepts. Replay access is a named
/// phase, never a separate flag that can outlive sealing or invalidation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionState {
    /// Calls are admitted, but reconstruction handles are revoked.
    Open,
    /// Calls are admitted, including calls through reconstruction handles.
    ReplayOpen,
    /// New calls are blocked while existing calls drain or lifecycle work runs. Reconstruction
    /// handles are revoked before a replay drain enters this phase.
    Transitioning,
    /// An ordinary transition during replay blocks new calls but retains reconstruction handles.
    /// Finishing this transition returns to `ReplayOpen`.
    ReplayTransitioning,
    /// New calls are permanently revoked for normal teardown.
    Sealed,
    /// A terminal failure permanently revoked new calls. Sealing cannot clear that failure.
    Invalidated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionEvent {
    BeginTransition,
    EnableReplayAccess,
    BeginReplayDrain,
    FinishTransition,
    Seal,
    Invalidate,
}

/// Gives admission after the event. A refused event leaves admission unchanged.
///
/// Ordinary transitions retain replay access and reopen only transitioning admission. A replay
/// drain requires open replay access and revokes it before entering the transition. Enabling
/// replay outside open admission and finishing outside a transition do nothing. Sealing revokes
/// replay access but preserves invalidation. Invalidation is terminal from every state.
fn transition(state: AdmissionState, event: AdmissionEvent) -> Result<AdmissionState, AccessError> {
    use AdmissionEvent as E;
    use AdmissionState as S;

    match (state, event) {
        (S::Open, E::BeginTransition) => Ok(S::Transitioning),
        (S::ReplayOpen, E::BeginTransition) => Ok(S::ReplayTransitioning),
        (S::Open | S::ReplayOpen, E::EnableReplayAccess) => Ok(S::ReplayOpen),
        (S::ReplayOpen, E::BeginReplayDrain) => Ok(S::Transitioning),
        (S::Open, E::BeginReplayDrain)
        | (S::Transitioning | S::ReplayTransitioning, E::BeginTransition | E::BeginReplayDrain) => {
            Err(AccessError::Transitioning)
        }
        (S::Sealed | S::Invalidated, E::BeginTransition | E::BeginReplayDrain) => {
            Err(AccessError::Revoked)
        }
        (S::Transitioning, E::FinishTransition) => Ok(S::Open),
        (S::ReplayTransitioning, E::FinishTransition) => Ok(S::ReplayOpen),
        (S::Invalidated, E::Seal) | (_, E::Invalidate) => Ok(S::Invalidated),
        (_, E::Seal) => Ok(S::Sealed),
        (state, E::EnableReplayAccess | E::FinishTransition) => Ok(state),
    }
}

/// Admission of one generation. The registry owns the lock; the state stays private so all
/// changes must pass through [`transition`].
pub(super) struct Admission {
    state: AdmissionState,
}

impl Admission {
    pub(super) fn new() -> Self {
        Self {
            state: AdmissionState::Open,
        }
    }

    fn apply(&mut self, event: AdmissionEvent) -> Result<(), AccessError> {
        self.state = transition(self.state, event)?;
        Ok(())
    }

    pub(super) fn admit_call(&self) -> Result<(), AccessError> {
        match self.state {
            AdmissionState::Open | AdmissionState::ReplayOpen => Ok(()),
            AdmissionState::Transitioning | AdmissionState::ReplayTransitioning => {
                Err(AccessError::Transitioning)
            }
            AdmissionState::Sealed | AdmissionState::Invalidated => Err(AccessError::Revoked),
        }
    }

    /// An open admitted before a transition can still return its node. After seal or
    /// invalidation it must close the node instead.
    pub(super) fn can_return_node(&self) -> bool {
        matches!(
            self.state,
            AdmissionState::Open
                | AdmissionState::ReplayOpen
                | AdmissionState::Transitioning
                | AdmissionState::ReplayTransitioning
        )
    }

    pub(super) fn replay_access_allowed(&self) -> bool {
        matches!(
            self.state,
            AdmissionState::ReplayOpen | AdmissionState::ReplayTransitioning
        )
    }

    pub(super) fn is_invalidated(&self) -> bool {
        self.state == AdmissionState::Invalidated
    }

    fn begin_transition(&mut self) -> Result<(), AccessError> {
        self.apply(AdmissionEvent::BeginTransition)
    }

    fn enable_replay_access(&mut self) {
        self.apply(AdmissionEvent::EnableReplayAccess)
            .expect("enabling replay access cannot fail");
    }

    fn begin_replay_drain(&mut self) -> Result<(), AccessError> {
        self.apply(AdmissionEvent::BeginReplayDrain)
    }

    fn finish_transition(&mut self) {
        self.apply(AdmissionEvent::FinishTransition)
            .expect("finishing admission transition cannot fail");
    }

    fn seal(&mut self) {
        self.apply(AdmissionEvent::Seal)
            .expect("sealing admission cannot fail");
    }

    fn invalidate(&mut self) {
        self.apply(AdmissionEvent::Invalidate)
            .expect("invalidating admission cannot fail");
    }
}

/// The sole right to finish a transition of this registry. It cannot be cloned or constructed
/// by callers. Dropping it leaves admission closed; it never reopens an unobserved transition.
#[must_use = "finish the transition to reopen admission, or seal the generation for cleanup"]
pub(super) struct AdmissionTransition {
    registry: Arc<GenerationRegistry>,
}

impl AdmissionTransition {
    /// Reopens admission only if no seal or invalidation has superseded this transition.
    pub(super) fn finish(self) {
        let mut state = self.registry.state.lock().unwrap();
        state.admission.finish_transition();
        self.registry.changed.notify_waiters();
    }
}

impl GenerationRegistry {
    pub(super) fn begin_transition(self: &Arc<Self>) -> Result<AdmissionTransition, AccessError> {
        self.state.lock().unwrap().admission.begin_transition()?;
        Ok(AdmissionTransition {
            registry: Arc::clone(self),
        })
    }

    pub(super) fn begin_replay_drain(self: &Arc<Self>) -> Result<AdmissionTransition, AccessError> {
        self.state.lock().unwrap().admission.begin_replay_drain()?;
        Ok(AdmissionTransition {
            registry: Arc::clone(self),
        })
    }

    pub(super) fn enable_replay_access(&self) {
        self.state.lock().unwrap().admission.enable_replay_access();
    }

    pub(super) fn seal(&self) {
        let mut state = self.state.lock().unwrap();
        state.admission.seal();
        self.changed.notify_waiters();
    }

    pub(super) fn invalidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.admission.invalidate();
        drop(state);
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
