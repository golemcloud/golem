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
    AttachmentController, BodyAdmissionState, LiveAttachmentPreparationRollback, OplogIndex,
    OwnerToolOperation, SerializableToolAdmissionRefusal, ToolAttachmentAdmissionRejection,
    ToolLiveAdmissionOutcome, ToolLiveAdmissionState, ToolOperationWinner, WorkerExecutorError,
};

impl LiveAttachmentPreparationRollback {
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LiveAttachmentPreparationRollback {
    fn drop(&mut self) {
        if self.armed {
            for attachment in &self.attachments {
                attachment.abort_prepared_live_memory_accounting();
            }
        }
    }
}

impl OwnerToolOperation {
    pub(crate) fn attach(
        &self,
        stdin: Option<AttachmentController>,
        stdout: Option<AttachmentController>,
        stderr: Option<AttachmentController>,
    ) -> bool {
        let mut state = self.owner.state.lock().unwrap();
        let operation = state
            .operations
            .get_mut(&self.id)
            .expect("owner tool operation must remain registered");
        if operation.winner.is_terminal() {
            drop(state);
            for attachment in stdin.iter().chain(stdout.iter()).chain(stderr.iter()) {
                attachment.fence_owner();
            }
            return false;
        }
        operation.stdin = stdin;
        operation.stdout = stdout;
        operation.stderr = stderr;
        tracing::debug!(
            operation_id = self.id,
            has_stdin = operation.stdin.is_some(),
            has_stdout = operation.stdout.is_some(),
            has_stderr = operation.stderr.is_some(),
            "Attached tool operation streams"
        );
        true
    }

    pub(crate) async fn activate_live_attachment_memory_accounting(
        &self,
    ) -> ToolLiveAdmissionOutcome {
        self.activate_live_attachment_memory_accounting_at(None)
            .await
    }

    pub(crate) async fn activate_live_attachment_memory_accounting_at(
        &self,
        boundary: Option<SerializableToolAdmissionRefusal>,
    ) -> ToolLiveAdmissionOutcome {
        let mut live_admission = self.live_admission.lock().await;
        match *live_admission {
            ToolLiveAdmissionState::Live => {
                return ToolLiveAdmissionOutcome::Admitted;
            }
            ToolLiveAdmissionState::ResourceExhaustedPending { .. }
            | ToolLiveAdmissionState::ResourceExhaustedHandled => {
                return ToolLiveAdmissionOutcome::ResourceExhausted;
            }
            ToolLiveAdmissionState::Historical => {}
        }

        let attachments = {
            let state = self.owner.state.lock().unwrap();
            let Some(operation) = state.operations.get(&self.id) else {
                return ToolLiveAdmissionOutcome::Fenced;
            };
            if state.admission_closed {
                return ToolLiveAdmissionOutcome::Fenced;
            }
            match operation.winner {
                ToolOperationWinner::Open => {}
                ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled => {
                    return ToolLiveAdmissionOutcome::Cancelled;
                }
                _ => return ToolLiveAdmissionOutcome::Fenced,
            }
            operation
                .stdin
                .iter()
                .chain(operation.stdout.iter())
                .chain(operation.stderr.iter())
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut rollback = LiveAttachmentPreparationRollback {
            attachments: attachments.clone(),
            armed: true,
        };
        for attachment in &attachments {
            if !attachment.prepare_live_memory_accounting().await {
                let outcome = self.select_admission_refusal(&mut live_admission, boundary);
                if outcome == ToolLiveAdmissionOutcome::ResourceExhausted {
                    rollback.disarm();
                }
                return outcome;
            }
        }

        let state = self.owner.state.lock().unwrap();
        let Some(operation) = state.operations.get(&self.id) else {
            return ToolLiveAdmissionOutcome::Fenced;
        };
        if state.admission_closed {
            return ToolLiveAdmissionOutcome::Fenced;
        }
        match operation.winner {
            ToolOperationWinner::Open => {}
            ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled => {
                return ToolLiveAdmissionOutcome::Cancelled;
            }
            _ => return ToolLiveAdmissionOutcome::Fenced,
        }
        for attachment in &attachments {
            attachment.commit_live_memory_accounting();
        }
        if !attachments.is_empty() {
            *live_admission = ToolLiveAdmissionState::Live;
        }
        rollback.disarm();
        ToolLiveAdmissionOutcome::Admitted
    }

    pub(super) fn select_admission_refusal(
        &self,
        live_admission: &mut ToolLiveAdmissionState,
        boundary: Option<SerializableToolAdmissionRefusal>,
    ) -> ToolLiveAdmissionOutcome {
        let mut state = self.owner.state.lock().unwrap();
        if state.admission_closed {
            return ToolLiveAdmissionOutcome::Fenced;
        }
        let Some(operation) = state.operations.get_mut(&self.id) else {
            return ToolLiveAdmissionOutcome::Fenced;
        };
        match operation.winner {
            ToolOperationWinner::Open => {
                self.admission_refusal.lock().unwrap().observed = boundary;
                operation.winner = ToolOperationWinner::SelectingOrdinary;
                operation
                    .winner_tx
                    .send_replace(ToolOperationWinner::SelectingOrdinary);
                *live_admission =
                    ToolLiveAdmissionState::ResourceExhaustedPending { preselected: true };
                ToolLiveAdmissionOutcome::ResourceExhausted
            }
            ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled => {
                ToolLiveAdmissionOutcome::Cancelled
            }
            _ => ToolLiveAdmissionOutcome::Fenced,
        }
    }

    pub(crate) fn install_recorded_admission_refusal(
        &self,
        refusal: SerializableToolAdmissionRefusal,
        terminal_index: OplogIndex,
    ) {
        self.admission_refusal.lock().unwrap().expected = Some((refusal, terminal_index));
    }

    pub(crate) fn observed_admission_refusal(&self) -> Option<SerializableToolAdmissionRefusal> {
        self.admission_refusal.lock().unwrap().observed.clone()
    }

    pub(crate) fn expected_admission_refusal(
        &self,
    ) -> Option<(SerializableToolAdmissionRefusal, OplogIndex)> {
        self.admission_refusal.lock().unwrap().expected.clone()
    }

    /// The caller must first prefer any recorded effect for this demand. A concurrent missing
    /// demand waits for the recorded first refusal, rather than changing which call observes it.
    pub(crate) async fn reject_missing_replay_admission(
        &self,
        boundary: &SerializableToolAdmissionRefusal,
    ) -> Result<(), WorkerExecutorError> {
        let mut changes = self.subscribe();
        let Some((expected, _)) = self.expected_admission_refusal() else {
            return Ok(());
        };
        loop {
            let winner = changes.borrow_and_update().clone();
            if &expected == boundary || self.observed_admission_refusal().is_some() {
                return Err(self.reproduce_admission_refusal(expected).await);
            }
            match winner {
                ToolOperationWinner::Open => {}
                ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled => {
                    return Err(
                        crate::durable_host::tool_attachment_live_admission_cancelled_error(),
                    );
                }
                _ => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "recorded tool admission refusal",
                        "tool settled before its recorded refusal was observed",
                    ));
                }
            }
            changes.changed().await.map_err(|_| {
                WorkerExecutorError::runtime(
                    "tool admission observer closed before recorded refusal",
                )
            })?;
        }
    }

    pub(crate) async fn reproduce_admission_refusal(
        &self,
        refusal: SerializableToolAdmissionRefusal,
    ) -> WorkerExecutorError {
        let mut admission = self.live_admission.lock().await;
        let outcome = if matches!(
            *admission,
            ToolLiveAdmissionState::ResourceExhaustedPending { .. }
                | ToolLiveAdmissionState::ResourceExhaustedHandled
        ) {
            ToolLiveAdmissionOutcome::ResourceExhausted
        } else {
            let state = self.owner.state.lock().unwrap();
            match state.operations.get(&self.id) {
                Some(operation) if state.owner_winner.is_none() => match operation.winner {
                    ToolOperationWinner::Open => {
                        let mut evidence = self.admission_refusal.lock().unwrap();
                        evidence.observed = Some(refusal);
                        evidence.historical_observed = true;
                        // Reconstruction can still fail in an owned child. Do not reserve
                        // its parent's terminal while that child must elect owner failure.
                        *admission =
                            ToolLiveAdmissionState::ResourceExhaustedPending { preselected: false };
                        operation.winner_tx.send_replace(operation.winner.clone());
                        self.owner.changed.notify_waiters();
                        ToolLiveAdmissionOutcome::ResourceExhausted
                    }
                    ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled => {
                        ToolLiveAdmissionOutcome::Cancelled
                    }
                    _ => ToolLiveAdmissionOutcome::Fenced,
                },
                _ => ToolLiveAdmissionOutcome::Fenced,
            }
        };
        match outcome {
            ToolLiveAdmissionOutcome::ResourceExhausted => {
                crate::durable_host::tool_attachment_live_admission_rejected_error()
            }
            ToolLiveAdmissionOutcome::Cancelled => {
                crate::durable_host::tool_attachment_live_admission_cancelled_error()
            }
            _ => WorkerExecutorError::runtime(
                "Tool operation was fenced during recorded admission refusal",
            ),
        }
    }

    pub(crate) async fn take_live_attachment_admission_rejection(
        &self,
    ) -> Option<ToolAttachmentAdmissionRejection> {
        let mut live_admission = self.live_admission.lock().await;
        if let ToolLiveAdmissionState::ResourceExhaustedPending { preselected } = *live_admission {
            *live_admission = ToolLiveAdmissionState::ResourceExhaustedHandled;
            Some(ToolAttachmentAdmissionRejection { preselected })
        } else {
            None
        }
    }

    pub(crate) async fn has_pending_live_admission_rejection(&self) -> bool {
        matches!(
            *self.live_admission.lock().await,
            ToolLiveAdmissionState::ResourceExhaustedPending { .. }
        )
    }

    pub(crate) async fn live_attachment_admission_was_rejected(&self) -> bool {
        matches!(
            *self.live_admission.lock().await,
            ToolLiveAdmissionState::ResourceExhaustedPending { .. }
                | ToolLiveAdmissionState::ResourceExhaustedHandled
        )
    }

    pub(crate) fn complete_rejected_live_attachment_memory_accounting(&self) {
        let attachments = {
            let state = self.owner.state.lock().unwrap();
            state
                .operations
                .get(&self.id)
                .map(|operation| {
                    operation
                        .stdin
                        .iter()
                        .chain(operation.stdout.iter())
                        .chain(operation.stderr.iter())
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        for attachment in attachments {
            attachment.complete_rejected_live_memory_accounting();
        }
    }

    pub(crate) fn admission_if_active(&self) -> Option<BodyAdmissionState> {
        self.owner
            .state
            .lock()
            .unwrap()
            .operations
            .get(&self.id)
            .map(|operation| operation.admission)
    }

    pub(crate) fn transition_admission(
        &self,
        expected: BodyAdmissionState,
        next: BodyAdmissionState,
    ) -> bool {
        let valid = matches!(
            (expected, next),
            (BodyAdmissionState::Staging, BodyAdmissionState::Ready)
                | (BodyAdmissionState::Staging, BodyAdmissionState::Running)
                | (BodyAdmissionState::Registered, BodyAdmissionState::Running)
                | (
                    BodyAdmissionState::Staging | BodyAdmissionState::Ready,
                    BodyAdmissionState::SettledWithoutBody
                )
        );
        if !valid {
            return false;
        }
        let mut state = self.owner.state.lock().unwrap();
        let operation = &mut state.operations.get_mut(&self.id).unwrap();
        if operation.admission != expected || operation.winner.is_terminal() {
            return false;
        }
        operation.admission = next;
        tracing::debug!(
            operation_id = self.id,
            previous_admission = ?expected,
            admission = ?next,
            "Transitioned tool body admission"
        );
        true
    }
}
