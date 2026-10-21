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
    Arc, CancellationEffect, EntityInvocationId, LaneOwnership, Ordering, OwnerFailureCleanupOwner,
    OwnerFailureCleanupState, OwnerFailureCleanupToken, OwnerFailureWinner, OwnerToolOperation,
    OwnerToolOperationContext, ProvisionalOwnerToolOperation, SerializableToolOperationTerminal,
    ToolOperationWinner, TrapType, WorkerExecutorError, drain_lane_ownerships, watch,
};

impl ProvisionalOwnerToolOperation {
    pub(crate) fn context(&self) -> &OwnerToolOperationContext {
        &self.context
    }

    pub(crate) fn is_rejected(&self) -> bool {
        self.operation.is_none()
    }

    pub(crate) fn accept(
        mut self,
        invocation_id: EntityInvocationId,
    ) -> Option<OwnerToolOperation> {
        let operation = self.operation.as_ref()?;
        if !operation.accept(invocation_id) {
            return None;
        }
        self.operation.take()
    }
}

impl Drop for ProvisionalOwnerToolOperation {
    fn drop(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        let mut state = operation.owner.state.lock().unwrap();
        if state.operations[&operation.id].invocation_id.is_none() {
            state.operations.remove(&operation.id);
            drop(state);
            #[cfg(test)]
            operation
                .owner
                .operation_removals
                .fetch_add(1, Ordering::Relaxed);
            operation.owner.changed.notify_waiters();
            tracing::debug!(
                operation_id = operation.id,
                "Removed unaccepted tool operation"
            );
        }
    }
}

impl Clone for OwnerToolOperation {
    fn clone(&self) -> Self {
        self.lease.handles.fetch_add(1, Ordering::Relaxed);
        Self {
            id: self.id,
            context: self.context.clone(),
            owner: self.owner.clone(),
            _winner: self._winner.clone(),
            lease: self.lease.clone(),
            live_admission: self.live_admission.clone(),
            admission_refusal: self.admission_refusal.clone(),
            cancellation_effect: self.cancellation_effect.clone(),
        }
    }
}

impl Drop for OwnerToolOperation {
    fn drop(&mut self) {
        let previous_handles = self.lease.handles.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous_handles > 0, "tool operation lease underflow");
        if previous_handles != 1 {
            return;
        }
        let mut state = self.owner.state.lock().unwrap();
        let removable = state.operations.get(&self.id).is_some_and(|operation| {
            operation.winner.is_terminal()
                && matches!(operation.lane, LaneOwnership::None)
                && operation.lease.handles.load(Ordering::Acquire) == 0
        });
        if removable {
            state.operations.remove(&self.id);
            drop(state);
            #[cfg(test)]
            self.owner
                .operation_removals
                .fetch_add(1, Ordering::Relaxed);
            self.owner.changed.notify_waiters();
        }
    }
}

impl OwnerToolOperation {
    pub(in crate::durable_host::tool) fn set_cancellation_effect(
        &self,
        effect: Box<CancellationEffect>,
    ) {
        assert!(self.cancellation_effect.set(effect).is_ok());
    }

    pub(super) fn accept(&self, invocation_id: EntityInvocationId) -> bool {
        let mut state = self.owner.state.lock().unwrap();
        let operation = state
            .operations
            .get_mut(&self.id)
            .expect("owner tool operation must remain registered");
        if operation.winner.is_terminal() || operation.invocation_id.is_some() {
            return false;
        }
        let start_index = invocation_id.start_index();
        operation.invocation_id = Some(invocation_id);
        tracing::debug!(
            operation_id = self.id,
            invocation_start_index = start_index.as_u64(),
            "Accepted durable tool operation"
        );
        true
    }

    pub(crate) fn invocation_id(&self) -> Option<EntityInvocationId> {
        self.owner
            .state
            .lock()
            .unwrap()
            .operations
            .get(&self.id)
            .and_then(|operation| operation.invocation_id.clone())
    }

    pub(crate) fn begin_cancel(&self) -> bool {
        let attachments = {
            let mut state = self.owner.state.lock().unwrap();
            if state.admission_closed || self.admission_refusal.lock().unwrap().historical_observed
            {
                return false;
            }
            let Some(operation) = state.operations.get_mut(&self.id) else {
                return false;
            };
            match operation.winner {
                ToolOperationWinner::Open => {
                    operation.winner = ToolOperationWinner::SelectingCancelled;
                    operation
                        .winner_tx
                        .send_replace(ToolOperationWinner::SelectingCancelled);
                }
                ToolOperationWinner::SelectingCancelled => {}
                _ => return false,
            }
            operation
                .stdin
                .iter()
                .chain(operation.stdout.iter())
                .chain(operation.stderr.iter())
                .cloned()
                .collect::<Vec<_>>()
        };
        for attachment in attachments {
            if let Some(effect) = self.cancellation_effect.get() {
                effect(self, &attachment);
            } else {
                let _ = attachment.cancel();
            }
        }
        tracing::debug!(
            operation_id = self.id,
            "Selected tool operation cancellation"
        );
        self.owner.changed.notify_waiters();
        true
    }

    pub(crate) fn begin_ordinary(&self) -> bool {
        self.begin_selection(ToolOperationWinner::SelectingOrdinary)
    }

    pub(crate) fn cancellation_selected_if_active(&self) -> bool {
        self.winner_if_active().is_some_and(|winner| {
            matches!(
                winner,
                ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled
            )
        })
    }

    pub(crate) fn claim_local_cancellation_interruption(&self) -> bool {
        let _ = self.begin_cancel();
        self.cancellation_selected_if_active()
    }

    /// Atomically elects this operation's guest trap as the owner-generation failure and fences
    /// every sibling that is still open. An ordinary/cancellation terminal selection already in
    /// progress must resolve before trap election is attempted.
    pub(crate) async fn select_trap(&self, trap: TrapType) -> bool {
        loop {
            let wait = self.owner.changed.notified();
            let selected = {
                let mut state = self.owner.state.lock().unwrap();
                if state.admission_closed {
                    return false;
                }
                if matches!(
                    state.operations[&self.id].winner,
                    ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled
                ) {
                    return false;
                }
                if state
                    .operations
                    .values()
                    .any(|operation| operation.winner.is_selecting())
                {
                    None
                } else if !matches!(state.operations[&self.id].winner, ToolOperationWinner::Open) {
                    return false;
                } else {
                    state.admission_closed = true;
                    state.owner_winner = Some(OwnerFailureWinner::Trap(trap.clone()));
                    state.owner_failure_cleanup = Some(OwnerFailureCleanupState {
                        owner: OwnerFailureCleanupOwner::Tool(self.id),
                        claimed: false,
                    });
                    let mut attachments = Vec::new();
                    for (id, operation) in state.operations.iter_mut() {
                        if matches!(operation.winner, ToolOperationWinner::Open) {
                            operation.winner = if *id == self.id {
                                ToolOperationWinner::Trap
                            } else {
                                ToolOperationWinner::FencedByOwner
                            };
                            operation.winner_tx.send_replace(operation.winner.clone());
                            attachments.extend(
                                operation
                                    .stdin
                                    .iter()
                                    .chain(operation.stdout.iter())
                                    .chain(operation.stderr.iter())
                                    .cloned(),
                            );
                        }
                    }
                    Some(attachments)
                }
            };
            if let Some(attachments) = selected {
                for attachment in attachments {
                    attachment.fence_owner();
                }
                tracing::debug!(
                    operation_id = self.id,
                    failure_kind = "trap",
                    "Selected tool owner failure"
                );
                self.owner.changed.notify_waiters();
                return true;
            }
            wait.await;
        }
    }

    pub(crate) async fn select_infrastructure(&self, error: WorkerExecutorError) -> bool {
        tracing::debug!(
            operation_id = self.id,
            error = %error,
            "Tool operation observed an infrastructure failure"
        );
        let selected = self
            .owner
            .select_owner_failure_with_cleanup(
                OwnerFailureWinner::Infrastructure(error),
                Some(OwnerFailureCleanupOwner::Tool(self.id)),
            )
            .await;
        tracing::debug!(
            operation_id = self.id,
            selected,
            "Tool operation classified an infrastructure failure"
        );
        selected
    }

    pub(crate) async fn select_failure(&self, winner: OwnerFailureWinner) -> bool {
        self.owner
            .select_owner_failure_with_cleanup(
                winner,
                Some(OwnerFailureCleanupOwner::Tool(self.id)),
            )
            .await
    }

    pub(crate) fn claim_owner_failure_cleanup(&self) -> Option<OwnerFailureCleanupToken> {
        self.owner
            .claim_owner_failure_cleanup(OwnerFailureCleanupOwner::Tool(self.id))
    }

    pub(super) fn begin_selection(&self, selecting: ToolOperationWinner) -> bool {
        let mut state = self.owner.state.lock().unwrap();
        if state.admission_closed {
            return false;
        }
        let Some(operation) = state.operations.get_mut(&self.id) else {
            return false;
        };
        if !matches!(operation.winner, ToolOperationWinner::Open) {
            return false;
        }
        operation.winner = selecting.clone();
        operation.winner_tx.send_replace(selecting);
        true
    }

    pub(crate) async fn resolve_cancel(&self, committed: bool) {
        self.resolve_selection(
            ToolOperationWinner::SelectingCancelled,
            committed.then_some(ToolOperationWinner::Cancelled),
        )
        .await;
    }

    pub(crate) async fn resolve_ordinary(
        &self,
        terminal: Arc<SerializableToolOperationTerminal>,
        committed: bool,
    ) {
        self.resolve_selection(
            ToolOperationWinner::SelectingOrdinary,
            committed.then_some(ToolOperationWinner::Ordinary {
                _terminal: terminal,
            }),
        )
        .await;
    }

    pub(super) async fn resolve_selection(
        &self,
        expected: ToolOperationWinner,
        committed: Option<ToolOperationWinner>,
    ) {
        let terminal_committed = committed.is_some();
        let next = committed.unwrap_or(ToolOperationWinner::Open);
        let lane = {
            let mut state = self.owner.state.lock().unwrap();
            let operation = state.operations.get_mut(&self.id).unwrap();
            assert_eq!(
                std::mem::discriminant(&operation.winner),
                std::mem::discriminant(&expected),
                "owner tool operation selection resolved from the wrong state"
            );
            operation.winner = next.clone();
            operation.winner_tx.send_replace(next);
            tracing::debug!(
                operation_id = self.id,
                terminal_committed,
                winner = operation.winner.kind_label(),
                "Resolved tool operation terminal selection"
            );
            if terminal_committed {
                std::mem::replace(&mut operation.lane, LaneOwnership::None)
            } else {
                LaneOwnership::None
            }
        };
        drain_lane_ownerships(vec![lane]).await;
        tracing::debug!(
            operation_id = self.id,
            "Drained tool terminal lane ownership"
        );
        self.owner.changed.notify_waiters();
    }

    pub(crate) fn winner_if_active(&self) -> Option<ToolOperationWinner> {
        self.owner
            .state
            .lock()
            .unwrap()
            .operations
            .get(&self.id)
            .map(|operation| operation.winner.clone())
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<ToolOperationWinner> {
        self._winner.subscribe()
    }

    pub(crate) async fn settle(self) {
        let lane = {
            let mut state = self.owner.state.lock().unwrap();
            let Some(operation) = state.operations.remove(&self.id) else {
                return;
            };
            assert!(
                operation.winner.is_terminal(),
                "an owner tool operation can only settle after terminal selection"
            );
            #[cfg(test)]
            self.owner
                .operation_removals
                .fetch_add(1, Ordering::Relaxed);
            operation.lane
        };
        self.owner.changed.notify_waiters();
        drain_lane_ownerships(vec![lane]).await;
        tracing::debug!(operation_id = self.id, "Settled tool operation resources");
    }
}
