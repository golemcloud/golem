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

//! Caller-owned run slots and the rules for admitting or withdrawing a store call.

use futures::future::BoxFuture;
use std::fmt::{Display, Formatter};

/// Why the limiter of a call withdrew the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Withdrawal {
    /// The caller stopped the call, or the shutdown came.
    Stopped,
    /// The deadline of the waits of the call passed.
    Deadline,
}

/// How a withdrawal ends a store call whose last failed run gave `F`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WithdrawnCall<F> {
    /// The call gives `Failed` with the failure of its last run.
    Failed(F),
    /// The call gives `Stopped` with the cause.
    Stopped(Withdrawal),
}

/// Gives how a withdrawal with `cause` ends a call whose last run failed on the storage with
/// `last`, or that has no failed run. The deadline gives `Failed` with `last` when there is one;
/// each other case gives `Stopped` with the cause.
pub(crate) fn withdrawn_call<F>(cause: Withdrawal, last: Option<F>) -> WithdrawnCall<F> {
    match (cause, last) {
        (Withdrawal::Deadline, Some(last)) => WithdrawnCall::Failed(last),
        (cause, _) => WithdrawnCall::Stopped(cause),
    }
}

/// The slots of one store call, from the limiter of the caller. The store takes one slot for each
/// run of the call, before the run starts, and gives it back when the run ends.
pub(crate) trait RunSlots: Send + Sync {
    /// Gives a slot, or the withdrawal of the call. `immediate` is true for the first take of the
    /// call and for a take that no wait came before.
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>>;

    /// Completes when the caller withdraws the call, with the cause.
    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal>;

    /// The call waits for its next run after a run that failed, and no write of the call can
    /// still land.
    fn waiting_after_failure(&self) {}

    /// The call only waits until its writes that can still land have landed or can no longer
    /// land. It holds no slot. It starts no run and no write until the limiter grants it a new
    /// slot.
    fn waiting_for_late_writes(&self) {}
}

/// A slot of a [`RunSlots`]. When it drops, it goes back to the limiter.
pub(crate) struct Slot(#[allow(dead_code)] Box<dyn Send + Sync>);

impl Slot {
    /// Gives the slot that `held` keeps. The slot goes back when `held` drops.
    pub(crate) fn new(held: impl Send + Sync + 'static) -> Self {
        Self(Box::new(held))
    }
}

impl Display for Withdrawal {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Stopped => "the filesystem snapshot call was stopped",
            Self::Deadline => "the deadline of the filesystem snapshot call passed",
        })
    }
}

/// A limiter that gives a slot to each take and never withdraws a call: the limiter of a `stat`,
/// which takes no slot, and of the calls that a test makes on a store outside the service.
pub(crate) struct Unlimited;

impl RunSlots for Unlimited {
    fn take(&self, _immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        Box::pin(std::future::ready(Ok(Slot::new(()))))
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(std::future::pending())
    }
}

#[cfg(test)]
mod tests;
