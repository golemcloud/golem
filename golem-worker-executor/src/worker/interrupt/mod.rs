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
use golem_service_base::error::worker_executor::InterruptKind;
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

    pub(crate) async fn claim_pending_terminal(&self) -> Option<PendingWorkerInterrupt> {
        self.lock().await.claim_pending_terminal()
    }

    /// Acknowledge only the published terminal interruption observed by the invocation.
    pub(crate) async fn claim_observed_terminal(&self, kind: InterruptKind) -> bool {
        let mut state = self.lock().await;
        if matches!(&*state, WorkerInterruptState::Pending(pending)
            if pending.kind == kind && pending.is_terminal())
        {
            state.claim_pending_terminal().is_some()
        } else {
            false
        }
    }

    /// Releases a claimed terminal request, for a new generation.
    pub(crate) async fn reset_terminal_for_new_generation(&self) {
        self.lock().await.reset_terminal_for_new_generation();
    }

    /// Whether a request waits or a terminal request is claimed.
    pub(crate) async fn has_interrupt(&self) -> bool {
        self.lock().await.has_interrupt()
    }

    pub(crate) async fn when_idle<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
        let state = self.lock().await;
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
mod tests;
