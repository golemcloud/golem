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
    AdmissionRefusalState, Arc, AtomicU64, AtomicUsize, AttachmentController, EntityInvocationId,
    FutureExt, HashMap, InterruptKind, Mutex, Notify, OperationLease, OperationState, Ordering,
    OwnerFailureCleanupOwner, OwnerFailureCleanupToken, OwnerFailureWinner, OwnerInvocationId,
    OwnerPhase, OwnerToolOperation, OwnerToolOperationContext, OwnerToolOperations,
    OwnerToolOperationsState, ProvisionalOwnerToolOperation, RegisteredOperation,
    ToolLiveAdmissionState, ToolOperationMetadata, ToolOperationSetMetadata, ToolOperationWinner,
    WorkerExecutorError, drain_lane_ownerships, watch,
};

impl OwnerToolOperations {
    /// Store tasks settle terminal selections. Keep driving them even if a different abandoned
    /// task returns an error, without dropping the in-progress owner election or lane drain.
    pub async fn fence_for_jump<T: Send + 'static>(
        self: &Arc<Self>,
        store: &mut wasmtime::StoreContextMut<'_, T>,
        mut on_error: impl FnMut(wasmtime::Error),
    ) {
        use wasmtime::AsContextMut;
        let operations = self.clone();
        let fence = async move {
            operations
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
                .await;
            operations.drain_owner_failure_lanes().await;
        }
        .boxed()
        .shared();
        loop {
            let pending = fence.clone();
            match store
                .as_context_mut()
                .run_concurrent(async move |_accessor| pending.await)
                .await
            {
                Ok(()) => return,
                Err(error) => on_error(error),
            }
        }
    }

    pub(crate) fn new() -> Arc<Self> {
        let failure_continuations = tokio_util::task::TaskTracker::new();
        failure_continuations.close();
        Arc::new(Self {
            next_id: AtomicU64::new(1),
            state: Mutex::new(OwnerToolOperationsState {
                phase: OwnerPhase::Open,
                operations: HashMap::new(),
            }),
            changed: Notify::new(),
            failure_continuations,
            #[cfg(test)]
            operation_removals: AtomicU64::new(0),
        })
    }

    pub(crate) fn create(
        self: &Arc<Self>,
        context: OwnerToolOperationContext,
    ) -> ProvisionalOwnerToolOperation {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let context = Arc::new(context);
        let lease = Arc::new(OperationLease {
            handles: AtomicUsize::new(1),
        });
        let winner_tx = {
            let mut state = self.state.lock().unwrap();
            if !state.phase.is_open() {
                drop(state);
                tracing::debug!(
                    operation_id = id,
                    call_mode = ?context.call_mode,
                    filesystem = ?context.activation.filesystem(),
                    "Rejected tool operation registration after owner failure"
                );
                return ProvisionalOwnerToolOperation {
                    context,
                    operation: None,
                };
            }
            let (winner_tx, _) = watch::channel(ToolOperationWinner::Open);
            state.operations.insert(
                id,
                RegisteredOperation {
                    context: context.clone(),
                    lease: lease.clone(),
                    invocation_id: None,
                    state: OperationState::new(winner_tx.clone()),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    execution: std::sync::Weak::new(),
                },
            );
            winner_tx
        };
        tracing::debug!(
            operation_id = id,
            call_mode = ?context.call_mode,
            filesystem = ?context.activation.filesystem(),
            "Registered tool operation"
        );
        ProvisionalOwnerToolOperation {
            context: context.clone(),
            operation: Some(OwnerToolOperation {
                id,
                context,
                owner: self.clone(),
                _winner: winner_tx,
                lease,
                live_admission: Arc::new(tokio::sync::Mutex::new(
                    ToolLiveAdmissionState::Historical,
                )),
                admission_refusal: Arc::new(Mutex::new(AdmissionRefusalState::default())),
                cancellation_effect: Arc::new(std::sync::OnceLock::new()),
            }),
        }
    }

    pub(crate) fn begin_generation(&self) -> Result<(), WorkerExecutorError> {
        if !self.failure_continuations.is_empty() {
            return Err(WorkerExecutorError::runtime(
                "tool failure continuation is still running",
            ));
        }
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.phase.cleanup_task() {
            task.clone().now_or_never().ok_or_else(|| {
                WorkerExecutorError::runtime("tool failure cleanup is still running")
            })??;
        }
        if !state.operations.is_empty() {
            return Err(WorkerExecutorError::runtime(
                "cannot begin an owner generation while tool operations are still active",
            ));
        }
        state.phase = OwnerPhase::Open;
        Ok(())
    }

    pub(crate) async fn select_owner_failure(&self, winner: OwnerFailureWinner) -> bool {
        self.select_owner_failure_with_cleanup(winner, None).await
    }

    /// Closes physical admission without electing a logical invocation or lifecycle failure.
    pub(crate) async fn fence_for_unload(&self) {
        self.select_owner_fence(None, None).await;
    }

    pub(crate) async fn select_primary_infrastructure(
        self: &Arc<Self>,
        error: WorkerExecutorError,
    ) -> Option<OwnerFailureCleanupToken> {
        if self
            .select_owner_failure_with_cleanup(
                OwnerFailureWinner::Infrastructure(error),
                Some(OwnerFailureCleanupOwner::Primary),
            )
            .await
        {
            self.claim_owner_failure_cleanup(OwnerFailureCleanupOwner::Primary)
        } else {
            None
        }
    }

    pub(super) fn claim_owner_failure_cleanup(
        self: &Arc<Self>,
        owner: OwnerFailureCleanupOwner,
    ) -> Option<OwnerFailureCleanupToken> {
        self.state
            .lock()
            .unwrap()
            .phase
            .claim_cleanup(owner, self.clone())
    }

    pub(super) async fn select_owner_failure_with_cleanup(
        &self,
        winner: OwnerFailureWinner,
        cleanup_owner: Option<OwnerFailureCleanupOwner>,
    ) -> bool {
        self.select_owner_fence(Some(winner), cleanup_owner).await
    }

    pub(super) async fn select_owner_fence(
        &self,
        winner: Option<OwnerFailureWinner>,
        cleanup_owner: Option<OwnerFailureCleanupOwner>,
    ) -> bool {
        loop {
            let wait = self.changed.notified();
            let selected = {
                let mut state = self.state.lock().unwrap();
                if state.phase.winner().is_some() || (winner.is_none() && !state.phase.is_open()) {
                    return false;
                }
                if let Some(OwnerFailureCleanupOwner::Tool(id)) = cleanup_owner
                    && let Some(operation) = state.operations.get_mut(&id)
                {
                    operation
                        .state
                        .withdraw_uncommitted_cancellation(&self.changed);
                }
                if state
                    .operations
                    .values()
                    .any(|operation| operation.state.winner().is_selecting())
                {
                    None
                } else {
                    let changed = match &winner {
                        Some(winner) => state.phase.select_failure(winner.clone(), cleanup_owner),
                        None => state.phase.fence_for_unload(),
                    };
                    if !changed {
                        return false;
                    }
                    let mut attachments = Vec::new();
                    for operation in state.operations.values_mut() {
                        if operation.state.fence_by_owner(false) {
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
                    failure_kind = winner
                        .as_ref()
                        .map_or("unload", OwnerFailureWinner::kind_label),
                    "Fenced tool owner"
                );
                self.changed.notify_waiters();
                return true;
            }
            wait.await;
        }
    }

    pub(crate) async fn drain_owner_failure_lanes(&self) {
        let lanes = {
            let mut state = self.state.lock().unwrap();
            if state.phase.is_open() {
                return;
            }
            state
                .operations
                .values_mut()
                .map(|operation| operation.state.take_lane_for_drain())
                .collect()
        };
        drain_lane_ownerships(lanes).await;
        let _removed = {
            let mut state = self.state.lock().unwrap();
            let previous_count = state.operations.len();
            state.operations.retain(|_, operation| {
                !matches!(
                    operation.state.winner(),
                    ToolOperationWinner::Trap | ToolOperationWinner::FencedByOwner
                ) || operation.lease.handles.load(Ordering::Acquire) != 0
            });
            previous_count - state.operations.len()
        };
        #[cfg(test)]
        self.operation_removals
            .fetch_add(_removed as u64, Ordering::Relaxed);
        self.changed.notify_waiters();
    }

    pub(crate) async fn wait_owner_settled(&self) {
        loop {
            let changed = self.changed.notified();
            if self.state.lock().unwrap().operations.is_empty() {
                return;
            }
            changed.await;
        }
    }

    /// Register before a failed publication receipt is released, even before failure election.
    pub(crate) fn track_failure_continuation(
        &self,
    ) -> tokio_util::task::task_tracker::TaskTrackerToken {
        self.failure_continuations.token()
    }

    pub(crate) async fn join_owner_failure_cleanup(&self) -> Result<(), WorkerExecutorError> {
        self.failure_continuations.wait().await;
        let task = self.state.lock().unwrap().phase.cleanup_task().cloned();
        if let Some(task) = task {
            task.await?;
        }
        Ok(())
    }

    pub(crate) fn close_failed_attachments(&self) {
        let attachments = self
            .state
            .lock()
            .unwrap()
            .operations
            .values()
            .filter(|operation| {
                matches!(
                    operation.state.winner(),
                    ToolOperationWinner::Trap | ToolOperationWinner::FencedByOwner
                )
            })
            .flat_map(|operation| {
                operation
                    .stdin
                    .iter()
                    .chain(operation.stdout.iter())
                    .chain(operation.stderr.iter())
                    .cloned()
            })
            .collect::<Vec<_>>();
        for attachment in attachments {
            attachment.fence_owner();
        }
    }

    #[cfg(test)]
    pub(super) fn owner_winner(&self) -> Option<OwnerFailureWinner> {
        self.selected_owner_failure()
    }

    pub(crate) fn selected_owner_failure(&self) -> Option<OwnerFailureWinner> {
        self.state.lock().unwrap().phase.winner().cloned()
    }

    pub(crate) fn interruptible_owner_failure(&self) -> Option<OwnerFailureWinner> {
        let state = self.state.lock().unwrap();
        // Primary-origin cleanup can need tasks in the primary Store. Stop guest execution
        // first; parent settlement and the retained cleanup join still fence completion/reuse.
        state.phase.interruptible_failure().cloned()
    }

    pub(crate) fn commit_if_owner_open(&self, commit: impl FnOnce()) -> bool {
        let state = self.state.lock().unwrap();
        if !state.phase.is_open() {
            return false;
        }
        commit();
        true
    }

    pub(crate) async fn wait_for_owner_failure(&self) -> OwnerFailureWinner {
        loop {
            let changed = self.changed.notified();
            if let Some(winner) = self.selected_owner_failure() {
                return winner;
            }
            changed.await;
        }
    }

    pub(crate) fn has_active_operations(&self) -> bool {
        !self.state.lock().unwrap().operations.is_empty()
    }

    pub(crate) fn metadata(&self) -> ToolOperationSetMetadata {
        let state = self.state.lock().unwrap();
        let mut operations = state
            .operations
            .iter()
            .map(|(operation_id, operation)| {
                let (admission, lane, winner) = operation.state.metadata();
                ToolOperationMetadata {
                    operation_id: *operation_id,
                    start_index: operation
                        .invocation_id
                        .as_ref()
                        .map(EntityInvocationId::start_index),
                    call_mode: operation.context.call_mode,
                    filesystem: operation.context.activation.filesystem(),
                    admission,
                    lane,
                    winner,
                    attachment_count: usize::from(operation.stdin.is_some())
                        + usize::from(operation.stdout.is_some())
                        + usize::from(operation.stderr.is_some()),
                    stdin: operation.stdin.as_ref().map(AttachmentController::metadata),
                    stdout: operation
                        .stdout
                        .as_ref()
                        .map(AttachmentController::metadata),
                    stderr: operation
                        .stderr
                        .as_ref()
                        .map(AttachmentController::metadata),
                }
            })
            .collect::<Vec<_>>();
        operations.sort_by_key(|operation| operation.operation_id);
        ToolOperationSetMetadata {
            owner_failure_selected: state.phase.winner().is_some(),
            owner_failure: state.phase.winner().map(OwnerFailureWinner::metadata),
            operations,
        }
    }

    /// Cancels unfinished descendants, traversing bodies whose terminal is already selected.
    /// A selected body can still own pending children; its outcome must not prevent their cleanup.
    pub(crate) fn cancel_children(&self, parent: &OwnerInvocationId) {
        let children = {
            let state = self.state.lock().unwrap();
            let mut parents = vec![parent.clone()];
            let mut children = Vec::new();
            while let Some(parent) = parents.pop() {
                for operation in state.operations.values() {
                    if operation.context.parent != parent {
                        continue;
                    }
                    if let Some(invocation) = &operation.invocation_id {
                        parents.push(OwnerInvocationId::Entity(invocation.clone()));
                    }
                    if let Some(execution) = operation.execution.upgrade() {
                        children.push(execution);
                    }
                }
            }
            children
        };
        for child in children {
            child.cancel_body();
        }
    }

    pub(crate) async fn wait_parent_settled(&self, parent: &OwnerInvocationId) {
        loop {
            let changed = self.changed.notified();
            if !self
                .state
                .lock()
                .unwrap()
                .operations
                .values()
                .any(|operation| &operation.context.parent == parent)
            {
                return;
            }
            changed.await;
        }
    }
}
