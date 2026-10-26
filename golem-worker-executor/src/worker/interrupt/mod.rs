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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) fn current_execution_interrupt_signal(
        &self,
    ) -> Option<Arc<tokio::sync::broadcast::Sender<InterruptKind>>> {
        match &*self.execution_status.read().unwrap() {
            ExecutionStatus::Running {
                interrupt_signal, ..
            } => Some(interrupt_signal.clone()),
            _ => None,
        }
    }
    /// Marks the worker as interrupting - this should eventually make the worker interrupted.
    /// There are several interruption modes but not all of them are supported by all worker
    /// executor implementations.
    ///
    /// - `Interrupt` means that the worker should be interrupted as soon as possible, and it should
    ///   remain interrupted.
    /// - `Restart` is a simulated crash, the worker gets automatically restarted after it got interrupted,
    ///   but only if the worker context supports recovering workers.
    /// - `Suspend` means that the worker should be moved out of memory and stay in suspended state,
    ///   automatically resumed when the worker is needed again. This only works if the worker context
    ///   supports recovering workers.
    pub async fn set_interrupting(
        self: &Arc<Self>,
        interrupt_kind: InterruptKind,
    ) -> Result<Option<Receiver<()>>, WorkerExecutorError> {
        self.set_interrupting_internal(
            interrupt_kind,
            false,
            UnloadReason::from_interrupt(interrupt_kind),
            false,
        )
        .await
    }
    pub(in crate::worker) async fn set_interrupting_internal(
        self: &Arc<Self>,
        interrupt_kind: InterruptKind,
        reacquire_permits: bool,
        unload_reason: UnloadReason,
        allow_deleting: bool,
    ) -> Result<Option<Receiver<()>>, WorkerExecutorError> {
        let Some((pending, was_running)) = self
            .queue_interrupt(
                interrupt_kind,
                reacquire_permits,
                unload_reason,
                allow_deleting,
            )
            .await
        else {
            return Ok(None);
        };
        self.start_interrupt_establishment(pending, was_running)
            .await
            .expect("worker interruption establishment task failed")
    }
    pub(in crate::worker) fn start_interrupt_establishment(
        self: &Arc<Self>,
        pending: PendingWorkerInterrupt,
        was_running: bool,
    ) -> JoinHandle<Result<Option<Receiver<()>>, WorkerExecutorError>> {
        let worker = self.clone();
        let establishment = pending.establishment.clone();
        let scope = self.release_scope();
        let receipt = scope.accepted_driver();
        tokio::spawn(async move {
            let result = worker.establish_queued_interrupt(pending, &scope).await;
            if result.is_err() {
                scope.fail(
                    crate::metrics::resource_release::Stage::EndToEnd,
                    crate::metrics::resource_release::Failure::StopDriver,
                );
            }
            establishment.complete();
            receipt.complete();
            if !was_running {
                scope.seal();
            }
            result
        })
    }
    pub(in crate::worker) async fn establish_queued_interrupt(
        self: &Arc<Self>,
        pending: PendingWorkerInterrupt,
        scope: &crate::metrics::resource_release::ReleaseScope,
    ) -> Result<Option<Receiver<()>>, WorkerExecutorError> {
        let interrupt_kind = pending.kind;
        let establishment = pending.establishment.clone();
        establishment.wait_for_predecessor().await;
        let active_agent = self.active_agent_for_this_owner().await;
        if let Some(active_agent) = &active_agent {
            active_agent
                .begin_fence_entity_bodies(OwnerFailureWinner::Lifecycle(interrupt_kind))
                .await;
        }
        let monthly = self
            .monthly_stop
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(kind, _)| *kind == interrupt_kind)
            .map(|(_, resource)| *resource);
        let cause = match monthly {
            Some(crate::services::resource_limits::MonthlyResourceExhaustion::Compute) => {
                crate::metrics::resource_release::Cause::MonthlyCompute
            }
            Some(crate::services::resource_limits::MonthlyResourceExhaustion::Memory) => {
                crate::metrics::resource_release::Cause::MonthlyMemory
            }
            Some(crate::services::resource_limits::MonthlyResourceExhaustion::DurableStorage) => {
                crate::metrics::resource_release::Cause::MonthlyDurableStorage
            }
            Some(crate::services::resource_limits::MonthlyResourceExhaustion::EphemeralStorage) => {
                crate::metrics::resource_release::Cause::MonthlyEphemeralStorage
            }
            None if interrupt_kind == InterruptKind::Jump => {
                crate::metrics::resource_release::Cause::Jump
            }
            None => pending.unload_request.reason.release_cause(),
        };
        scope.freeze_cause(cause);
        let receiver = self.notify_queued_interrupt(interrupt_kind).await;
        if let Some(active_agent) = active_agent {
            active_agent.drain_fenced_entity_bodies().await;
        }
        Ok(receiver)
    }
    pub(in crate::worker) async fn notify_queued_interrupt(
        &self,
        interrupt_kind: InterruptKind,
    ) -> Option<Receiver<()>> {
        let instance_guard = self.instance.lock().await;
        if let WorkerInstance::Running(running) = instance_guard.deletion_runtime() {
            let _ = running.sender.send(WorkerCommand::WorkAvailable);
        }
        drop(instance_guard);
        let mut status = self.execution_status.write().unwrap();
        let signal = match &*status {
            ExecutionStatus::Loading {
                interrupt_signal, ..
            }
            | ExecutionStatus::Running {
                interrupt_signal, ..
            } => Some(interrupt_signal.clone()),
            _ => None,
        };
        let receipt = match &*status {
            ExecutionStatus::Interrupting {
                await_interruption, ..
            } => await_interruption.clone(),
            _ => Arc::new(tokio::sync::broadcast::channel(1).0),
        };
        let receiver = receipt.subscribe();
        status
            .publish_interrupt(interrupt_kind, receipt, signal.as_ref())
            .then_some(receiver)
    }
    pub(in crate::worker) async fn queue_interrupt(
        &self,
        interrupt_kind: InterruptKind,
        reacquire_permits: bool,
        unload_reason: UnloadReason,
        allow_deleting: bool,
    ) -> Option<(PendingWorkerInterrupt, bool)> {
        loop {
            let lifecycle = self.instance.lock().await;
            if !allow_deleting && lifecycle.ensure_not_deleting().is_err() {
                return None;
            }
            if let Some(mut interrupts) = self.interrupt_signal.try_lock() {
                let pending = register_interrupt_request(
                    &mut interrupts,
                    &self.interrupt_establishment_tail,
                    interrupt_kind,
                    reacquire_permits,
                    UnloadRequest::ordinary(unload_reason),
                )?;
                let was_running =
                    matches!(lifecycle.deletion_runtime(), WorkerInstance::Running(_));
                return Some((pending, was_running));
            }
            drop(lifecycle);
            tokio::task::yield_now().await;
        }
    }
    pub(crate) async fn acknowledge_invocation_interrupt(&self, kind: InterruptKind) {
        let observed = matches!(&*self.execution_status.read().unwrap(),
            ExecutionStatus::Interrupting { interrupt_kind, .. } if *interrupt_kind == kind);
        if observed {
            self.interrupt_signal.claim_observed_terminal(kind).await;
        }
    }
    /// Waits for all previously accepted interrupts to finish fencing and draining entity bodies.
    /// This does not wait for the primary Store to unload or for final filesystem cleanup.
    pub async fn join_interrupt_establishment(&self) -> Result<(), WorkerExecutorError> {
        let establishment = self.interrupt_establishment_tail.lock().unwrap().clone();
        establishment.wait().await;
        self.monthly_cleanup_failure().map_or(Ok(()), Err)
    }
    pub(crate) async fn set_interrupting_for(
        self: &Arc<Self>,
        interrupt_kind: InterruptKind,
        unload_reason: UnloadReason,
    ) -> Result<Option<Receiver<()>>, WorkerExecutorError> {
        self.set_interrupting_internal(interrupt_kind, false, unload_reason, false)
            .await
    }
    /// A receiver of whether a terminal interrupt request waits for this worker.
    pub fn terminal_interrupt(&self) -> tokio::sync::watch::Receiver<bool> {
        self.interrupt_signal.terminal()
    }
}

/// Queue and advance the existing establishment tail in the same locked step.
pub(in crate::worker) fn register_interrupt_request(
    interrupts: &mut interrupt::InterruptGuard<'_>,
    tail: &StdMutex<Arc<InterruptEstablishment>>,
    kind: InterruptKind,
    reacquire_permits: bool,
    unload_request: UnloadRequest,
) -> Option<PendingWorkerInterrupt> {
    let mut tail = tail.lock().unwrap();
    let pending = PendingWorkerInterrupt {
        kind,
        reacquire_permits,
        unload_request,
        establishment: Arc::new(InterruptEstablishment::new(tail.clone())),
    };
    interrupts.queue(pending.clone()).then(|| {
        *tail = pending.establishment.clone();
        pending
    })
}

impl InterruptEstablishment {
    pub(in crate::worker) fn new(predecessor: Arc<Self>) -> Self {
        Self {
            predecessor: StdMutex::new(Some(predecessor)),
            complete: AtomicBool::new(false),
            completed: tokio::sync::Notify::new(),
        }
    }

    pub(in crate::worker) fn ready() -> Arc<Self> {
        Arc::new(Self {
            predecessor: StdMutex::new(None),
            complete: AtomicBool::new(true),
            completed: tokio::sync::Notify::new(),
        })
    }

    pub(in crate::worker) fn complete(&self) {
        self.complete.store(true, Ordering::Release);
        self.completed.notify_waiters();
    }

    pub(in crate::worker) async fn wait(&self) {
        while !self.complete.load(Ordering::Acquire) {
            let notified = self.completed.notified();
            if self.complete.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }

    pub(in crate::worker) async fn wait_for_predecessor(&self) {
        let predecessor = self.predecessor.lock().unwrap().clone();
        if let Some(predecessor) = predecessor {
            predecessor.wait().await;
            self.predecessor.lock().unwrap().take();
        }
    }
}

impl PendingWorkerInterrupt {
    pub(in crate::worker) fn is_terminal(&self) -> bool {
        !matches!(self.kind, InterruptKind::Restart | InterruptKind::Jump)
    }

    /// How the invocation loop should proceed after honoring this interrupt. A suspension retains
    /// its timestamp so a newer wakeup can supersede it, while an explicit interrupt remains
    /// terminal.
    pub(in crate::worker) fn retry_decision(&self) -> RetryDecision {
        if self.reacquire_permits {
            RetryDecision::ReacquirePermits
        } else {
            match self.kind {
                InterruptKind::EntityCancellation => {
                    unreachable!("entity cancellation cannot interrupt the owner")
                }
                InterruptKind::Restart | InterruptKind::Jump => RetryDecision::Immediate,
                InterruptKind::Interrupt(_) | InterruptKind::ShardLost => RetryDecision::None,
                InterruptKind::Suspend(timestamp) => RetryDecision::TryStop(timestamp),
            }
        }
    }
}

impl WorkerInterruptState {
    pub(in crate::worker) fn has_interrupt(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Whether a terminal request waits and nobody has taken it.
    pub(in crate::worker) fn terminal_pending(&self) -> bool {
        matches!(self, Self::Pending(interrupt) if interrupt.is_terminal())
    }

    pub(in crate::worker) fn queue(&mut self, mut interrupt: PendingWorkerInterrupt) -> bool {
        match self {
            Self::TerminalClaimed if interrupt.is_terminal() => {
                *self = Self::Pending(interrupt);
                true
            }
            Self::TerminalClaimed => false,
            Self::Pending(current) if current.is_terminal() => false,
            Self::Pending(current) => {
                if matches!(interrupt.kind, InterruptKind::Restart) {
                    interrupt.reacquire_permits |= current.reacquire_permits;
                }
                *self = Self::Pending(interrupt);
                true
            }
            Self::Idle => {
                *self = Self::Pending(interrupt);
                true
            }
        }
    }

    pub(in crate::worker) fn take(&mut self) -> Option<PendingWorkerInterrupt> {
        match std::mem::take(self) {
            Self::Pending(interrupt) if interrupt.is_terminal() => {
                *self = Self::TerminalClaimed;
                Some(interrupt)
            }
            Self::Pending(interrupt) => Some(interrupt),
            state => {
                *self = state;
                None
            }
        }
    }

    pub(in crate::worker) fn claim_pending_terminal(&mut self) -> Option<PendingWorkerInterrupt> {
        match self {
            Self::Pending(interrupt) if interrupt.is_terminal() => self.take(),
            _ => None,
        }
    }

    pub(in crate::worker) fn reset_terminal_for_new_generation(&mut self) {
        if matches!(self, Self::TerminalClaimed) {
            *self = Self::Idle;
        }
    }
}
