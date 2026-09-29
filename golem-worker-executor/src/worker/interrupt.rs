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

//! The interrupt requests of a worker, and whether a terminal one waits.
//!
//! Each change of the requests goes through a method of [`Interrupts`]. The method publishes
//! whether a terminal request waits before it releases the lock, so a reader of
//! [`Interrupts::terminal`] never sees an older value than the last change that completed.

use super::{PendingWorkerInterrupt, WorkerInterruptState};
use async_lock::Mutex;
use tokio::sync::watch;

/// The interrupt requests of one worker.
#[derive(Debug)]
pub(crate) struct Interrupts {
    state: Mutex<WorkerInterruptState>,
    /// Whether a terminal request waits.
    terminal: watch::Sender<bool>,
}

impl Default for Interrupts {
    fn default() -> Self {
        Self {
            state: Mutex::new(WorkerInterruptState::default()),
            terminal: watch::channel(false).0,
        }
    }
}

impl Interrupts {
    /// Queues `interrupt` when the lock is free. Gives `None` when another holder has the lock,
    /// and otherwise whether the request was queued.
    pub(crate) fn try_queue(&self, interrupt: PendingWorkerInterrupt) -> Option<bool> {
        let mut state = self.state.try_lock()?;
        let queued = state.queue(interrupt);
        self.publish(&state);
        Some(queued)
    }

    /// Takes the waiting request. A terminal request stays claimed until the generation stops.
    pub(crate) async fn take(&self) -> Option<PendingWorkerInterrupt> {
        let mut state = self.state.lock().await;
        let taken = state.take();
        self.publish(&state);
        taken
    }

    /// Takes the waiting request when it is terminal.
    pub(crate) async fn claim_pending_terminal(&self) -> Option<PendingWorkerInterrupt> {
        let mut state = self.state.lock().await;
        let claimed = state.claim_pending_terminal();
        self.publish(&state);
        claimed
    }

    /// Releases a claimed terminal request, for a new generation.
    pub(crate) async fn reset_terminal_for_new_generation(&self) {
        let mut state = self.state.lock().await;
        state.reset_terminal_for_new_generation();
        self.publish(&state);
    }

    /// Whether a request waits or a terminal request is claimed.
    pub(crate) async fn has_interrupt(&self) -> bool {
        self.state.lock().await.has_interrupt()
    }

    /// Runs `f` while the lock is held and no request waits or is claimed. Gives `None` without
    /// a call of `f` otherwise.
    pub(crate) async fn when_idle<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock().await;
        (!state.has_interrupt()).then(f)
    }

    /// Whether a terminal request waits now.
    pub(crate) fn terminal_pending(&self) -> bool {
        *self.terminal.borrow()
    }

    /// A receiver of whether a terminal request waits.
    pub(crate) fn terminal(&self) -> watch::Receiver<bool> {
        self.terminal.subscribe()
    }

    fn publish(&self, state: &WorkerInterruptState) {
        let pending = state.terminal_pending();
        self.terminal.send_if_modified(|terminal| {
            let changed = *terminal != pending;
            *terminal = pending;
            changed
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{UnloadReason, UnloadRequest};
    use golem_common::model::Timestamp;
    use golem_service_base::error::worker_executor::InterruptKind;
    use test_r::test;

    fn interrupt(kind: InterruptKind) -> PendingWorkerInterrupt {
        PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
        }
    }

    #[test]
    async fn each_change_publishes_whether_a_terminal_request_waits() {
        let interrupts = Interrupts::default();
        let terminal = interrupts.terminal();

        let restart = (
            interrupts.try_queue(interrupt(InterruptKind::Restart)),
            interrupts.terminal_pending(),
        );
        let taken_restart = interrupts.take().await.is_some();
        let suspend = (
            interrupts.try_queue(interrupt(InterruptKind::Suspend(Timestamp::now_utc()))),
            *terminal.borrow(),
        );
        let claimed = (
            interrupts.claim_pending_terminal().await.is_some(),
            interrupts.terminal_pending(),
            interrupts.has_interrupt().await,
        );
        let queued_again = (
            interrupts.try_queue(interrupt(InterruptKind::Interrupt(Timestamp::now_utc()))),
            interrupts.terminal_pending(),
        );
        let taken = (
            interrupts.take().await.is_some(),
            interrupts.terminal_pending(),
        );
        interrupts.reset_terminal_for_new_generation().await;
        let reset = (
            interrupts.has_interrupt().await,
            interrupts.terminal_pending(),
        );

        assert_eq!(restart, (Some(true), false));
        assert!(taken_restart);
        assert_eq!(suspend, (Some(true), true));
        assert_eq!(claimed, (true, false, true));
        assert_eq!(queued_again, (Some(true), true));
        assert_eq!(taken, (true, false));
        assert_eq!(reset, (false, false));
    }

    #[test]
    async fn a_receiver_wakes_only_when_whether_a_terminal_request_waits_changes() {
        let interrupts = Interrupts::default();
        let terminal = interrupts.terminal();

        interrupts.try_queue(interrupt(InterruptKind::Restart));
        let after_restart = terminal.has_changed().unwrap();
        interrupts.take().await;
        interrupts.try_queue(interrupt(InterruptKind::Suspend(Timestamp::now_utc())));
        let after_suspend = terminal.has_changed().unwrap();

        assert_eq!((after_restart, after_suspend), (false, true));
    }

    #[test]
    async fn a_queue_while_the_lock_is_held_gives_none_and_changes_nothing() {
        let interrupts = Interrupts::default();
        let held = interrupts.state.lock().await;

        let queued =
            interrupts.try_queue(interrupt(InterruptKind::Interrupt(Timestamp::now_utc())));
        drop(held);

        assert_eq!(queued, None);
        assert!(!interrupts.terminal_pending());
        assert!(!interrupts.has_interrupt().await);
    }

    #[test]
    async fn when_idle_runs_only_without_a_request() {
        let interrupts = Interrupts::default();
        let idle = interrupts.when_idle(|| 1).await;
        interrupts.try_queue(interrupt(InterruptKind::Restart));
        let busy = interrupts.when_idle(|| 2).await;

        assert_eq!((idle, busy), (Some(1), None));
    }
}
