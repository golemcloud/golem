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
//! Each change of the requests holds an [`InterruptGuard`]. The guard publishes whether a
//! terminal request waits before it releases the lock, so a reader of [`Interrupts::terminal`]
//! never sees an older value than the last change that completed.

use super::{PendingWorkerInterrupt, WorkerInterruptState};
use async_lock::{Mutex, MutexGuard};
use std::ops::{Deref, DerefMut};
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

pub(super) struct InterruptGuard<'a> {
    state: MutexGuard<'a, WorkerInterruptState>,
    interrupts: &'a Interrupts,
}

impl Deref for InterruptGuard<'_> {
    type Target = WorkerInterruptState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for InterruptGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Drop for InterruptGuard<'_> {
    fn drop(&mut self) {
        self.interrupts.publish(&self.state);
    }
}

impl Interrupts {
    pub(super) async fn lock(&self) -> InterruptGuard<'_> {
        InterruptGuard {
            state: self.state.lock().await,
            interrupts: self,
        }
    }

    pub(super) fn try_lock(&self) -> Option<InterruptGuard<'_>> {
        Some(InterruptGuard {
            state: self.state.try_lock()?,
            interrupts: self,
        })
    }

    /// Takes the published request. A terminal request stays claimed until the generation stops.
    pub(crate) async fn take(&self) -> Option<PendingWorkerInterrupt> {
        self.lock().await.take()
    }

    /// Releases a claimed terminal request, for a new generation.
    pub(crate) async fn reset_terminal_for_new_generation(&self) {
        self.lock().await.reset_terminal_for_new_generation();
    }

    /// Whether a request waits or a terminal request is claimed.
    pub(crate) async fn has_interrupt(&self) -> bool {
        self.lock().await.has_interrupt()
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

        let queued_restart = interrupts
            .try_lock()
            .unwrap()
            .queue(interrupt(InterruptKind::Restart));
        let restart = (queued_restart, interrupts.terminal_pending());
        {
            let mut state = interrupts.lock().await;
            state.freeze();
            state.publish();
        }
        let taken_restart = interrupts.take().await.is_some();
        let queued_suspend = interrupts
            .try_lock()
            .unwrap()
            .queue(interrupt(InterruptKind::Suspend(Timestamp::now_utc())));
        let suspend = (queued_suspend, *terminal.borrow());
        assert!(interrupts.take().await.is_none());
        interrupts.lock().await.freeze();
        assert!(interrupts.terminal_pending());
        interrupts.lock().await.publish();
        let claimed_request = interrupts.lock().await.claim_pending_terminal().is_some();
        let claimed = (
            claimed_request,
            interrupts.terminal_pending(),
            interrupts.has_interrupt().await,
        );
        let queued_interrupt = interrupts
            .try_lock()
            .unwrap()
            .queue(interrupt(InterruptKind::Interrupt(Timestamp::now_utc())));
        let queued_again = (queued_interrupt, interrupts.terminal_pending());
        {
            let mut state = interrupts.lock().await;
            state.freeze();
            state.publish();
        }
        let taken = (
            interrupts.take().await.is_some(),
            interrupts.terminal_pending(),
        );
        interrupts.reset_terminal_for_new_generation().await;
        let reset = (
            interrupts.has_interrupt().await,
            interrupts.terminal_pending(),
        );

        assert_eq!(restart, (true, false));
        assert!(taken_restart);
        assert_eq!(suspend, (true, true));
        assert_eq!(claimed, (true, false, true));
        assert_eq!(queued_again, (true, true));
        assert_eq!(taken, (true, false));
        assert_eq!(reset, (false, false));
    }

    #[test]
    async fn a_receiver_wakes_only_when_whether_a_terminal_request_waits_changes() {
        let interrupts = Interrupts::default();
        let terminal = interrupts.terminal();

        interrupts
            .try_lock()
            .unwrap()
            .queue(interrupt(InterruptKind::Restart));
        let after_restart = terminal.has_changed().unwrap();
        {
            let mut state = interrupts.lock().await;
            state.freeze();
            state.publish();
        }
        interrupts.take().await;
        interrupts
            .try_lock()
            .unwrap()
            .queue(interrupt(InterruptKind::Suspend(Timestamp::now_utc())));
        let after_suspend = terminal.has_changed().unwrap();

        assert_eq!((after_restart, after_suspend), (false, true));
    }

    #[test]
    async fn a_queue_while_the_lock_is_held_gives_none_and_changes_nothing() {
        let interrupts = Interrupts::default();
        let held = interrupts.lock().await;

        let queued = interrupts.try_lock().map(|mut state| {
            state.queue(interrupt(InterruptKind::Interrupt(Timestamp::now_utc())))
        });
        drop(held);

        assert_eq!(queued, None);
        assert!(!interrupts.terminal_pending());
        assert!(!interrupts.has_interrupt().await);
    }

    #[test]
    async fn a_rejected_request_keeps_the_terminal_projection() {
        let interrupts = Interrupts::default();
        let kind = InterruptKind::Suspend(Timestamp::now_utc());
        assert!(interrupts.lock().await.queue(interrupt(kind)));
        let terminal = interrupts.terminal();
        assert!(*terminal.borrow());
        assert!(
            !interrupts
                .lock()
                .await
                .queue(interrupt(InterruptKind::Restart))
        );
        assert!(*terminal.borrow());
        assert!(!terminal.has_changed().unwrap());
    }
}
