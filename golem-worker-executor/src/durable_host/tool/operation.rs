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

use super::attachment::{AttachmentController, ToolAttachmentMetadata};
use crate::model::TrapType;
use crate::worker::owner_lane::{
    OwnerInvocationId, OwnerInvocationPermit, OwnerInvocationTicket, OwnerLane, OwnerLaneWait,
};
use golem_common::model::agent::Principal;
use golem_common::model::entity::{
    EntityActivation, EntityCallMode, EntityInvocationDescriptor, EntityInvocationId,
    FilesystemCapability,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::payload::types::SerializableToolOperationTerminal;
use golem_common::schema::TypedSchemaValue;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, watch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BodyAdmissionState {
    Staging,
    Ready,
    Registered,
    Running,
    SettledWithoutBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolLiveAdmissionOutcome {
    Admitted,
    ResourceExhausted,
    Cancelled,
    Fenced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolLiveAdmissionState {
    Historical,
    Live,
    ResourceExhaustedPending,
    ResourceExhaustedHandled,
}

pub(crate) struct ToolAttachmentAdmissionRejection;

struct LiveAttachmentPreparationRollback {
    attachments: Vec<AttachmentController>,
    armed: bool,
}

impl LiveAttachmentPreparationRollback {
    fn disarm(&mut self) {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolBodyAdmissionMetadata {
    Staging,
    Ready,
    Registered,
    Running,
    SettledWithoutBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolOperationLaneMetadata {
    None,
    Queued,
    Acquiring,
    Granted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolOperationWinnerMetadata {
    Open,
    SelectingCancelled,
    SelectingOrdinary,
    Cancelled,
    Ordinary,
    Trap,
    FencedByOwner,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOperationMetadata {
    pub operation_id: u64,
    pub start_index: Option<OplogIndex>,
    pub call_mode: EntityCallMode,
    pub filesystem: FilesystemCapability,
    pub admission: ToolBodyAdmissionMetadata,
    pub lane: ToolOperationLaneMetadata,
    pub winner: ToolOperationWinnerMetadata,
    pub attachment_count: usize,
    pub stdin: Option<ToolAttachmentMetadata>,
    pub stdout: Option<ToolAttachmentMetadata>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolOwnerFailureMetadata {
    Trap,
    Lifecycle,
    Infrastructure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOperationSetMetadata {
    pub owner_failure_selected: bool,
    pub owner_failure: Option<ToolOwnerFailureMetadata>,
    pub operations: Vec<ToolOperationMetadata>,
}

#[derive(Clone, Debug)]
pub(crate) enum OwnerFailureWinner {
    Trap(TrapType),
    Lifecycle(InterruptKind),
    Infrastructure(WorkerExecutorError),
}

impl OwnerFailureWinner {
    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            Self::Trap(_) => "trap",
            Self::Lifecycle(_) => "lifecycle",
            Self::Infrastructure(_) => "infrastructure",
        }
    }

    fn metadata(&self) -> ToolOwnerFailureMetadata {
        match self {
            Self::Trap(_) => ToolOwnerFailureMetadata::Trap,
            Self::Lifecycle(_) => ToolOwnerFailureMetadata::Lifecycle,
            Self::Infrastructure(_) => ToolOwnerFailureMetadata::Infrastructure,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ToolOperationWinner {
    Open,
    SelectingCancelled,
    SelectingOrdinary,
    Cancelled,
    Ordinary {
        _terminal: Arc<SerializableToolOperationTerminal>,
    },
    Trap,
    FencedByOwner,
}

impl ToolOperationWinner {
    fn kind_label(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::SelectingCancelled => "selecting_cancelled",
            Self::SelectingOrdinary => "selecting_ordinary",
            Self::Cancelled => "cancelled",
            Self::Ordinary { .. } => "ordinary",
            Self::Trap => "trap",
            Self::FencedByOwner => "fenced_by_owner",
        }
    }

    fn is_selecting(&self) -> bool {
        matches!(self, Self::SelectingCancelled | Self::SelectingOrdinary)
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Ordinary { .. } | Self::Trap | Self::FencedByOwner
        )
    }
}

pub(crate) struct OwnerToolOperationContext {
    pub parent: OwnerInvocationId,
    pub call_mode: EntityCallMode,
    pub activation: Arc<EntityActivation>,
    pub calling_principal: Principal,
    pub principal: Principal,
    pub descriptor: EntityInvocationDescriptor,
    pub input: TypedSchemaValue,
}

struct RegisteredOperation {
    context: Arc<OwnerToolOperationContext>,
    lease: Arc<OperationLease>,
    invocation_id: Option<EntityInvocationId>,
    winner: ToolOperationWinner,
    winner_tx: watch::Sender<ToolOperationWinner>,
    admission: BodyAdmissionState,
    lane: LaneOwnership,
    acquisition_error: Option<String>,
    stdin: Option<AttachmentController>,
    stdout: Option<AttachmentController>,
}

struct OperationLease {
    handles: AtomicUsize,
}

enum LaneOwnership {
    None,
    Ticket(OwnerInvocationTicket),
    Acquiring(AcquisitionControl),
    Permit { _permit: OwnerInvocationPermit },
}

struct AcquisitionControl {
    abort: tokio::task::AbortHandle,
    drained: Arc<AcquisitionDrain>,
}

#[derive(Default)]
struct AcquisitionDrain {
    done: AtomicBool,
    notify: Notify,
}

impl AcquisitionDrain {
    fn finish(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        while !self.done.load(Ordering::Acquire) {
            let notified = self.notify.notified();
            if self.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }
}

struct AcquisitionDrainGuard {
    drained: Arc<AcquisitionDrain>,
    permit: Option<OwnerInvocationPermit>,
}

impl Drop for AcquisitionDrainGuard {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.finish();
    }
}

struct OwnerToolOperationsState {
    owner_winner: Option<OwnerFailureWinner>,
    owner_failure_cleanup: Option<OwnerFailureCleanupState>,
    owner_failure_cleanup_complete: bool,
    operations: HashMap<u64, RegisteredOperation>,
}

struct OwnerFailureCleanupState {
    operation_id: u64,
    claimed: bool,
}

pub(crate) struct OwnerFailureCleanupToken {
    operation_id: u64,
}

/// One arbitration domain for all accepted tool operations in an owner generation. Durable
/// terminal selection uses a two-step selecting state: an owner failure waits for that selection
/// to resolve, so an operation winner and owner winner cannot be chosen independently.
pub(crate) struct OwnerToolOperations {
    next_id: AtomicU64,
    state: Mutex<OwnerToolOperationsState>,
    changed: Notify,
    #[cfg(test)]
    operation_removals: AtomicU64,
}

impl std::fmt::Debug for OwnerToolOperations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerToolOperations")
            .field("next_id", &self.next_id.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl OwnerToolOperations {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            next_id: AtomicU64::new(1),
            state: Mutex::new(OwnerToolOperationsState {
                owner_winner: None,
                owner_failure_cleanup: None,
                owner_failure_cleanup_complete: false,
                operations: HashMap::new(),
            }),
            changed: Notify::new(),
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
            if state.owner_winner.is_some() {
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
                    winner: ToolOperationWinner::Open,
                    winner_tx: winner_tx.clone(),
                    admission: BodyAdmissionState::Staging,
                    lane: LaneOwnership::None,
                    acquisition_error: None,
                    stdin: None,
                    stdout: None,
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
            }),
        }
    }

    pub(crate) fn begin_generation(&self) -> Result<(), WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        if !state.operations.is_empty() {
            return Err(WorkerExecutorError::runtime(
                "cannot begin an owner generation while tool operations are still active",
            ));
        }
        state.owner_winner = None;
        state.owner_failure_cleanup = None;
        state.owner_failure_cleanup_complete = false;
        Ok(())
    }

    pub(crate) async fn select_owner_failure(&self, winner: OwnerFailureWinner) -> bool {
        self.select_owner_failure_with_cleanup(winner, None).await
    }

    async fn select_owner_failure_with_cleanup(
        &self,
        winner: OwnerFailureWinner,
        cleanup_operation_id: Option<u64>,
    ) -> bool {
        loop {
            let wait = self.changed.notified();
            let selected = {
                let mut state = self.state.lock().unwrap();
                if state.owner_winner.is_some() {
                    return false;
                }
                if state
                    .operations
                    .values()
                    .any(|operation| operation.winner.is_selecting())
                {
                    None
                } else {
                    state.owner_winner = Some(winner.clone());
                    state.owner_failure_cleanup =
                        cleanup_operation_id.map(|operation_id| OwnerFailureCleanupState {
                            operation_id,
                            claimed: false,
                        });
                    let mut attachments = Vec::new();
                    for operation in state.operations.values_mut() {
                        if matches!(operation.winner, ToolOperationWinner::Open) {
                            operation.winner = ToolOperationWinner::FencedByOwner;
                            operation
                                .winner_tx
                                .send_replace(ToolOperationWinner::FencedByOwner);
                            attachments.extend(
                                operation
                                    .stdin
                                    .iter()
                                    .chain(operation.stdout.iter())
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
                    failure_kind = winner.kind_label(),
                    "Selected tool owner failure"
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
            if state.owner_winner.is_none() {
                return;
            }
            state
                .operations
                .values_mut()
                .map(|operation| std::mem::replace(&mut operation.lane, LaneOwnership::None))
                .collect()
        };
        drain_lane_ownerships(lanes).await;
        let _removed = {
            let mut state = self.state.lock().unwrap();
            let previous_count = state.operations.len();
            state.operations.retain(|_, operation| {
                !matches!(
                    operation.winner,
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

    pub(crate) fn complete_owner_failure_cleanup(&self, token: OwnerFailureCleanupToken) {
        let mut state = self.state.lock().unwrap();
        assert!(
            state.owner_winner.is_some(),
            "owner failure cleanup requires a selected failure"
        );
        assert!(
            matches!(
                state.owner_failure_cleanup,
                Some(OwnerFailureCleanupState {
                    operation_id,
                    claimed: true,
                }) if operation_id == token.operation_id
            ),
            "owner failure cleanup requires its claimed operation token"
        );
        assert!(
            state.operations.is_empty(),
            "owner failure cleanup requires every tool operation to settle"
        );
        state.owner_failure_cleanup_complete = true;
        drop(state);
        self.changed.notify_waiters();
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
                    operation.winner,
                    ToolOperationWinner::Trap | ToolOperationWinner::FencedByOwner
                )
            })
            .flat_map(|operation| {
                operation
                    .stdin
                    .iter()
                    .chain(operation.stdout.iter())
                    .cloned()
            })
            .collect::<Vec<_>>();
        for attachment in attachments {
            attachment.fence_owner();
        }
    }

    #[cfg(test)]
    fn owner_winner(&self) -> Option<OwnerFailureWinner> {
        self.state.lock().unwrap().owner_winner.clone()
    }

    pub(crate) fn selected_owner_failure(&self) -> Option<OwnerFailureWinner> {
        self.state.lock().unwrap().owner_winner.clone()
    }

    pub(crate) fn interruptible_owner_failure(&self) -> Option<OwnerFailureWinner> {
        let state = self.state.lock().unwrap();
        state
            .owner_failure_cleanup_complete
            .then(|| state.owner_winner.clone())
            .flatten()
    }

    pub(crate) fn commit_if_owner_open(&self, commit: impl FnOnce()) -> bool {
        let state = self.state.lock().unwrap();
        if state.owner_winner.is_some() {
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
            .map(|(operation_id, operation)| ToolOperationMetadata {
                operation_id: *operation_id,
                start_index: operation
                    .invocation_id
                    .as_ref()
                    .map(EntityInvocationId::start_index),
                call_mode: operation.context.call_mode,
                filesystem: operation.context.activation.filesystem(),
                admission: match operation.admission {
                    BodyAdmissionState::Staging => ToolBodyAdmissionMetadata::Staging,
                    BodyAdmissionState::Ready => ToolBodyAdmissionMetadata::Ready,
                    BodyAdmissionState::Registered => ToolBodyAdmissionMetadata::Registered,
                    BodyAdmissionState::Running => ToolBodyAdmissionMetadata::Running,
                    BodyAdmissionState::SettledWithoutBody => {
                        ToolBodyAdmissionMetadata::SettledWithoutBody
                    }
                },
                lane: match &operation.lane {
                    LaneOwnership::None => ToolOperationLaneMetadata::None,
                    LaneOwnership::Ticket(_) => ToolOperationLaneMetadata::Queued,
                    LaneOwnership::Acquiring(_) => ToolOperationLaneMetadata::Acquiring,
                    LaneOwnership::Permit { .. } => ToolOperationLaneMetadata::Granted,
                },
                winner: match &operation.winner {
                    ToolOperationWinner::Open => ToolOperationWinnerMetadata::Open,
                    ToolOperationWinner::SelectingCancelled => {
                        ToolOperationWinnerMetadata::SelectingCancelled
                    }
                    ToolOperationWinner::SelectingOrdinary => {
                        ToolOperationWinnerMetadata::SelectingOrdinary
                    }
                    ToolOperationWinner::Cancelled => ToolOperationWinnerMetadata::Cancelled,
                    ToolOperationWinner::Ordinary { .. } => ToolOperationWinnerMetadata::Ordinary,
                    ToolOperationWinner::Trap => ToolOperationWinnerMetadata::Trap,
                    ToolOperationWinner::FencedByOwner => {
                        ToolOperationWinnerMetadata::FencedByOwner
                    }
                },
                attachment_count: usize::from(operation.stdin.is_some())
                    + usize::from(operation.stdout.is_some()),
                stdin: operation.stdin.as_ref().map(AttachmentController::metadata),
                stdout: operation
                    .stdout
                    .as_ref()
                    .map(AttachmentController::metadata),
            })
            .collect::<Vec<_>>();
        operations.sort_by_key(|operation| operation.operation_id);
        ToolOperationSetMetadata {
            owner_failure_selected: state.owner_winner.is_some(),
            owner_failure: state
                .owner_winner
                .as_ref()
                .map(OwnerFailureWinner::metadata),
            operations,
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

    pub(crate) fn register_ready_bodies(
        &self,
        lane: &OwnerLane,
        starts: &[OplogIndex],
        result_await_parent: Option<&OwnerInvocationId>,
    ) -> Result<(Vec<OwnerInvocationId>, Option<OwnerLaneWait>), WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        if state.owner_winner.is_some() {
            return Err(WorkerExecutorError::runtime(
                "owner generation was fenced before tool lane registration",
            ));
        }
        let mut starts = starts.to_vec();
        starts.sort_unstable();
        let registrations = starts
            .iter()
            .map(|start| {
                let (operation_id, operation) = state
                    .operations
                    .iter()
                    .find(|(_, operation)| {
                        operation
                            .invocation_id
                            .as_ref()
                            .is_some_and(|invocation| invocation.start_index() == *start)
                    })
                    .ok_or_else(|| {
                        WorkerExecutorError::runtime(format!(
                            "ready tool operation at {start} is no longer registered"
                        ))
                    })?;
                if operation.admission != BodyAdmissionState::Ready
                    || !matches!(operation.winner, ToolOperationWinner::Open)
                {
                    return Err(WorkerExecutorError::runtime(format!(
                        "tool operation at {start} is not ready for lane registration"
                    )));
                }
                let invocation_id = operation.invocation_id.clone().ok_or_else(|| {
                    WorkerExecutorError::runtime("tool operation was not durably accepted")
                })?;
                Ok((
                    *operation_id,
                    operation.context.parent.clone(),
                    invocation_id,
                    operation.context.call_mode,
                    operation.context.activation.filesystem(),
                ))
            })
            .collect::<Result<Vec<_>, WorkerExecutorError>>()?;
        let mut tickets = Vec::with_capacity(registrations.len());
        let mut invocations = Vec::with_capacity(registrations.len());
        for (operation_id, parent, invocation_id, call_mode, filesystem) in registrations {
            let ticket = lane
                .register_entity(parent, invocation_id.clone(), call_mode, filesystem)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            invocations.push(OwnerInvocationId::Entity(invocation_id));
            tickets.push((operation_id, ticket));
        }
        let wait = result_await_parent
            .filter(|_| !invocations.is_empty())
            .map(|parent| {
                lane.await_invocations(parent, invocations.clone())
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
            })
            .transpose()?;
        for (operation_id, ticket) in tickets {
            let operation = state
                .operations
                .get_mut(&operation_id)
                .expect("validated tool operation must remain registered while locked");
            operation.lane = LaneOwnership::Ticket(ticket);
            operation.admission = BodyAdmissionState::Registered;
        }
        drop(state);
        self.changed.notify_waiters();
        Ok((invocations, wait))
    }

    #[cfg(test)]
    fn operation_count(&self) -> usize {
        self.state.lock().unwrap().operations.len()
    }

    #[cfg(test)]
    fn operation_removal_count(&self) -> u64 {
        self.operation_removals.load(Ordering::Relaxed)
    }
}

pub(crate) struct ProvisionalOwnerToolOperation {
    context: Arc<OwnerToolOperationContext>,
    operation: Option<OwnerToolOperation>,
}

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

pub(crate) struct OwnerToolOperation {
    id: u64,
    context: Arc<OwnerToolOperationContext>,
    owner: Arc<OwnerToolOperations>,
    _winner: watch::Sender<ToolOperationWinner>,
    lease: Arc<OperationLease>,
    live_admission: Arc<tokio::sync::Mutex<ToolLiveAdmissionState>>,
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
    pub(crate) fn context(&self) -> &OwnerToolOperationContext {
        &self.context
    }

    pub(crate) fn attach(
        &self,
        stdin: Option<AttachmentController>,
        stdout: Option<AttachmentController>,
    ) -> bool {
        let mut state = self.owner.state.lock().unwrap();
        let operation = state
            .operations
            .get_mut(&self.id)
            .expect("owner tool operation must remain registered");
        if operation.winner.is_terminal() {
            drop(state);
            for attachment in stdin.iter().chain(stdout.iter()) {
                attachment.fence_owner();
            }
            return false;
        }
        operation.stdin = stdin;
        operation.stdout = stdout;
        tracing::debug!(
            operation_id = self.id,
            has_stdin = operation.stdin.is_some(),
            has_stdout = operation.stdout.is_some(),
            "Attached tool operation streams"
        );
        true
    }

    pub(crate) async fn activate_live_attachment_memory_accounting(
        &self,
    ) -> ToolLiveAdmissionOutcome {
        let mut live_admission = self.live_admission.lock().await;
        match *live_admission {
            ToolLiveAdmissionState::Live => {
                return ToolLiveAdmissionOutcome::Admitted;
            }
            ToolLiveAdmissionState::ResourceExhaustedPending
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
            if state.owner_winner.is_some() {
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
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut rollback = LiveAttachmentPreparationRollback {
            attachments: attachments.clone(),
            armed: true,
        };
        for attachment in &attachments {
            if !attachment.prepare_live_memory_accounting().await {
                let mut state = self.owner.state.lock().unwrap();
                if state.owner_winner.is_some() {
                    return ToolLiveAdmissionOutcome::Fenced;
                }
                let Some(operation) = state.operations.get_mut(&self.id) else {
                    return ToolLiveAdmissionOutcome::Fenced;
                };
                match operation.winner {
                    ToolOperationWinner::Open => {
                        operation.winner = ToolOperationWinner::SelectingOrdinary;
                        operation
                            .winner_tx
                            .send_replace(ToolOperationWinner::SelectingOrdinary);
                        *live_admission = ToolLiveAdmissionState::ResourceExhaustedPending;
                        drop(state);
                        rollback.disarm();
                        return ToolLiveAdmissionOutcome::ResourceExhausted;
                    }
                    ToolOperationWinner::SelectingCancelled | ToolOperationWinner::Cancelled => {
                        return ToolLiveAdmissionOutcome::Cancelled;
                    }
                    _ => return ToolLiveAdmissionOutcome::Fenced,
                }
            }
        }

        let state = self.owner.state.lock().unwrap();
        let Some(operation) = state.operations.get(&self.id) else {
            return ToolLiveAdmissionOutcome::Fenced;
        };
        if state.owner_winner.is_some() {
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

    pub(crate) async fn take_live_attachment_admission_rejection(
        &self,
    ) -> Option<ToolAttachmentAdmissionRejection> {
        let mut live_admission = self.live_admission.lock().await;
        if *live_admission == ToolLiveAdmissionState::ResourceExhaustedPending {
            *live_admission = ToolLiveAdmissionState::ResourceExhaustedHandled;
            Some(ToolAttachmentAdmissionRejection)
        } else {
            None
        }
    }

    pub(crate) async fn has_pending_live_admission_rejection(&self) -> bool {
        *self.live_admission.lock().await == ToolLiveAdmissionState::ResourceExhaustedPending
    }

    pub(crate) async fn live_attachment_admission_was_rejected(&self) -> bool {
        matches!(
            *self.live_admission.lock().await,
            ToolLiveAdmissionState::ResourceExhaustedPending
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
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        for attachment in attachments {
            attachment.complete_rejected_live_memory_accounting();
        }
    }

    fn accept(&self, invocation_id: EntityInvocationId) -> bool {
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

    #[cfg(test)]
    pub(crate) fn invocation_id(&self) -> Option<EntityInvocationId> {
        self.owner
            .state
            .lock()
            .unwrap()
            .operations
            .get(&self.id)
            .and_then(|operation| operation.invocation_id.clone())
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

    /// Registers with the unchanged GOL-33 owner lane and moves the returned ticket under operation
    /// arbitration before returning. No await or externally visible intermediate ownership exists.
    #[cfg(test)]
    pub(crate) fn register_body(&self, lane: &OwnerLane) -> Result<bool, WorkerExecutorError> {
        let mut state = self.owner.state.lock().unwrap();
        if state.owner_winner.is_some() {
            return Ok(false);
        }
        let operation = state.operations.get_mut(&self.id).unwrap();
        if operation.admission != BodyAdmissionState::Ready
            || !matches!(operation.winner, ToolOperationWinner::Open)
        {
            return Ok(false);
        }
        let invocation_id = operation.invocation_id.clone().ok_or_else(|| {
            WorkerExecutorError::runtime("tool operation was not durably accepted")
        })?;
        match lane.register_entity(
            self.context.parent.clone(),
            invocation_id,
            self.context.call_mode,
            self.context.activation.filesystem(),
        ) {
            Ok(ticket) => {
                operation.lane = LaneOwnership::Ticket(ticket);
                operation.admission = BodyAdmissionState::Registered;
                Ok(true)
            }
            Err(error) => Err(WorkerExecutorError::runtime(error.to_string())),
        }
    }

    /// Acquires an already-owned ticket. If cancellation or owner fencing won while the lane grant
    /// was pending, the granted permit is completed here and no guest body may start.
    pub(crate) async fn acquire_registered_body(&self) -> Result<bool, WorkerExecutorError> {
        let drained = Arc::new(AcquisitionDrain::default());
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = {
            let mut state = self.owner.state.lock().unwrap();
            let operation = state.operations.get_mut(&self.id).unwrap();
            if operation.admission != BodyAdmissionState::Registered {
                return Ok(false);
            }
            let LaneOwnership::Ticket(ticket) =
                std::mem::replace(&mut operation.lane, LaneOwnership::None)
            else {
                return Err(WorkerExecutorError::runtime(
                    "registered tool operation lost its lane ticket",
                ));
            };
            let owner = self.owner.clone();
            let operation_id = self.id;
            let task_drain = drained.clone();
            let drain = AcquisitionDrainGuard {
                drained: task_drain,
                permit: None,
            };
            let task = tokio::spawn(async move {
                let mut drain = drain;
                let _ = start_rx.await;
                match ticket.acquire().await {
                    Ok(permit) => drain.permit = Some(permit),
                    Err(error) => {
                        let mut state = owner.state.lock().unwrap();
                        let operation =
                            state.operations.get_mut(&operation_id).ok_or_else(|| {
                                WorkerExecutorError::runtime("acquiring tool operation was removed")
                            })?;
                        if matches!(operation.lane, LaneOwnership::Acquiring(_)) {
                            operation.lane = LaneOwnership::None;
                            operation.acquisition_error = Some(error.to_string());
                        }
                        return Ok::<(), WorkerExecutorError>(());
                    }
                }

                loop {
                    let wait = owner.changed.notified();
                    let should_wait = {
                        let mut state = owner.state.lock().unwrap();
                        let owner_open = state.owner_winner.is_none();
                        let operation =
                            state.operations.get_mut(&operation_id).ok_or_else(|| {
                                WorkerExecutorError::runtime("acquiring tool operation was removed")
                            })?;
                        if !matches!(operation.lane, LaneOwnership::Acquiring(_)) {
                            return Ok(());
                        }
                        if owner_open && matches!(operation.winner, ToolOperationWinner::Open) {
                            operation.lane = LaneOwnership::Permit {
                                _permit: drain
                                    .permit
                                    .take()
                                    .expect("acquired lane permit must remain task-owned"),
                            };
                            operation.admission = BodyAdmissionState::Running;
                            false
                        } else if owner_open && operation.winner.is_selecting() {
                            true
                        } else {
                            // A committed operation or owner failure will detach and abort this
                            // acquisition. If it won before this task observed the state change,
                            // release the permit here instead.
                            drop(drain.permit.take());
                            operation.lane = LaneOwnership::None;
                            false
                        }
                    };
                    if should_wait {
                        wait.await;
                    } else {
                        break;
                    }
                }
                Ok(())
            });
            operation.lane = LaneOwnership::Acquiring(AcquisitionControl {
                abort: task.abort_handle(),
                drained: drained.clone(),
            });
            task
        };
        let start_failed = start_tx.send(()).is_err();
        drained.wait().await;
        match task.await {
            Ok(result) => result?,
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                return Err(WorkerExecutorError::runtime(format!(
                    "tool lane acquisition task failed: {error}"
                )));
            }
        }
        let mut state = self.owner.state.lock().unwrap();
        let owner_open = state.owner_winner.is_none();
        let operation = state
            .operations
            .get_mut(&self.id)
            .ok_or_else(|| WorkerExecutorError::runtime("acquiring tool operation was removed"))?;
        if let Some(error) = operation.acquisition_error.take() {
            return Err(WorkerExecutorError::runtime(error));
        }
        if start_failed && !operation.winner.is_terminal() && owner_open {
            operation.lane = LaneOwnership::None;
            return Err(WorkerExecutorError::runtime(
                "tool lane acquisition failed to start",
            ));
        }
        Ok(operation.admission == BodyAdmissionState::Running
            && matches!(operation.lane, LaneOwnership::Permit { .. })
            && matches!(operation.winner, ToolOperationWinner::Open))
    }

    pub(crate) async fn wait_until_registered(&self) -> bool {
        enum RegistrationStatus {
            Waiting,
            Registered,
            Terminal,
        }
        loop {
            let changed = self.owner.changed.notified();
            let status = {
                let state = self.owner.state.lock().unwrap();
                match state.operations.get(&self.id) {
                    None => RegistrationStatus::Terminal,
                    Some(operation) if operation.winner.is_terminal() => {
                        RegistrationStatus::Terminal
                    }
                    Some(operation) if operation.admission == BodyAdmissionState::Registered => {
                        RegistrationStatus::Registered
                    }
                    Some(_) => RegistrationStatus::Waiting,
                }
            };
            match status {
                RegistrationStatus::Registered => return true,
                RegistrationStatus::Terminal => return false,
                RegistrationStatus::Waiting => changed.await,
            }
        }
    }

    pub(crate) fn begin_cancel(&self) -> bool {
        let attachments = {
            let mut state = self.owner.state.lock().unwrap();
            if state.owner_winner.is_some() {
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
                .cloned()
                .collect::<Vec<_>>()
        };
        for attachment in attachments {
            let _ = attachment.cancel();
        }
        tracing::debug!(
            operation_id = self.id,
            "Selected tool operation cancellation"
        );
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
                if state.owner_winner.is_some() {
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
                    state.owner_winner = Some(OwnerFailureWinner::Trap(trap.clone()));
                    state.owner_failure_cleanup = Some(OwnerFailureCleanupState {
                        operation_id: self.id,
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
            "Tool operation observed an infrastructure failure"
        );
        let selected = self
            .owner
            .select_owner_failure_with_cleanup(
                OwnerFailureWinner::Infrastructure(error),
                Some(self.id),
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
            .select_owner_failure_with_cleanup(winner, Some(self.id))
            .await
    }

    pub(crate) fn claim_owner_failure_cleanup(&self) -> Option<OwnerFailureCleanupToken> {
        let mut state = self.owner.state.lock().unwrap();
        let cleanup = state.owner_failure_cleanup.as_mut()?;
        if cleanup.operation_id != self.id || cleanup.claimed {
            return None;
        }
        cleanup.claimed = true;
        Some(OwnerFailureCleanupToken {
            operation_id: self.id,
        })
    }

    fn begin_selection(&self, selecting: ToolOperationWinner) -> bool {
        let mut state = self.owner.state.lock().unwrap();
        if state.owner_winner.is_some() {
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

    async fn resolve_selection(
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

    #[cfg(test)]
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

    #[cfg(test)]
    fn owns_lane_value_if_active(&self) -> Option<bool> {
        let state = self.owner.state.lock().unwrap();
        state
            .operations
            .get(&self.id)
            .map(|operation| !matches!(operation.lane, LaneOwnership::None))
    }

    #[cfg(test)]
    fn is_acquiring_lane_if_active(&self) -> Option<bool> {
        self.owner
            .state
            .lock()
            .unwrap()
            .operations
            .get(&self.id)
            .map(|operation| matches!(operation.lane, LaneOwnership::Acquiring(_)))
    }
}

async fn drain_lane_ownerships(lanes: Vec<LaneOwnership>) {
    for lane in lanes {
        if let LaneOwnership::Acquiring(control) = lane {
            control.abort.abort();
            control.drained.wait().await;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeferredAdmissionReadiness {
    Staging,
    Ready,
    SettledWithoutBody,
}

struct DeferredAdmission {
    readiness: DeferredAdmissionReadiness,
    cohort: Option<DeferredAdmissionCohort>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeferredAdmissionCohort {
    ResultAwait(OplogIndex),
    ParentEnd,
}

pub(crate) struct DeferredAdmissionTable {
    parents: Mutex<HashMap<OwnerInvocationId, BTreeMap<OplogIndex, DeferredAdmission>>>,
    closed_parents: Mutex<HashSet<OwnerInvocationId>>,
    changed: Notify,
}

impl Default for DeferredAdmissionTable {
    fn default() -> Self {
        Self {
            parents: Mutex::new(HashMap::new()),
            closed_parents: Mutex::new(HashSet::new()),
            changed: Notify::new(),
        }
    }
}

impl DeferredAdmissionTable {
    pub(crate) fn begin_generation(&self) -> Result<(), WorkerExecutorError> {
        let mut closed_parents = self.closed_parents.lock().unwrap();
        let parents = self.parents.lock().unwrap();
        if !parents.is_empty() {
            let staged_admissions = parents.values().map(BTreeMap::len).sum::<usize>();
            return Err(WorkerExecutorError::runtime(format!(
                "cannot begin an owner generation with {staged_admissions} deferred tool \
                     admissions across {} parents",
                parents.len()
            )));
        }
        closed_parents.clear();
        Ok(())
    }

    pub(crate) fn insert(&self, parent: OwnerInvocationId, start: OplogIndex) -> bool {
        let closed_parents = self.closed_parents.lock().unwrap();
        if closed_parents.contains(&parent) {
            return false;
        }
        self.parents
            .lock()
            .unwrap()
            .entry(parent)
            .or_default()
            .insert(
                start,
                DeferredAdmission {
                    readiness: DeferredAdmissionReadiness::Staging,
                    cohort: None,
                },
            )
            .is_none()
    }

    pub(crate) fn close_parent_and_snapshot(
        &self,
        parent: &OwnerInvocationId,
    ) -> Option<Vec<OplogIndex>> {
        let mut closed_parents = self.closed_parents.lock().unwrap();
        if !closed_parents.insert(parent.clone()) {
            return None;
        }
        let mut parents = self.parents.lock().unwrap();
        let starts = parents
            .get_mut(parent)
            .map(|entries| {
                for entry in entries.values_mut() {
                    entry.cohort = Some(DeferredAdmissionCohort::ParentEnd);
                }
                entries.keys().copied().collect()
            })
            .unwrap_or_default();
        Some(starts)
    }

    pub(crate) fn clear_closed_parent(&self, parent: &OwnerInvocationId) -> bool {
        let mut closed_parents = self.closed_parents.lock().unwrap();
        let parents = self.parents.lock().unwrap();
        if parents.contains_key(parent) {
            return false;
        }
        closed_parents.remove(parent)
    }

    pub(crate) fn settle_staging(
        &self,
        parent: &OwnerInvocationId,
        start: OplogIndex,
        readiness: DeferredAdmissionReadiness,
    ) -> bool {
        if readiness == DeferredAdmissionReadiness::Staging {
            return false;
        }
        let mut parents = self.parents.lock().unwrap();
        let Some(entry) = parents
            .get_mut(parent)
            .and_then(|entries| entries.get_mut(&start))
        else {
            return false;
        };
        if entry.readiness != DeferredAdmissionReadiness::Staging {
            return false;
        }
        entry.readiness = readiness;
        drop(parents);
        self.changed.notify_waiters();
        true
    }

    pub(crate) fn remove_settled_without_body(
        &self,
        parent: &OwnerInvocationId,
        start: OplogIndex,
    ) -> bool {
        let mut parents = self.parents.lock().unwrap();
        let removed = parents.get_mut(parent).is_some_and(|entries| {
            entries.get(&start).is_some_and(|entry| {
                entry.readiness == DeferredAdmissionReadiness::SettledWithoutBody
            }) && entries.remove(&start).is_some()
        });
        if parents.get(parent).is_some_and(BTreeMap::is_empty) {
            parents.remove(parent);
        }
        drop(parents);
        if removed {
            self.changed.notify_waiters();
        }
        removed
    }

    pub(crate) fn settle_operation_without_body(
        &self,
        parent: &OwnerInvocationId,
        start: OplogIndex,
        expected_readiness: DeferredAdmissionReadiness,
        operation: &OwnerToolOperation,
        expected_admission: BodyAdmissionState,
    ) -> bool {
        let _closed_parents = self.closed_parents.lock().unwrap();
        let mut parents = self.parents.lock().unwrap();
        let Some(entry) = parents
            .get_mut(parent)
            .and_then(|entries| entries.get_mut(&start))
        else {
            return false;
        };
        if entry.readiness != expected_readiness
            || !operation
                .transition_admission(expected_admission, BodyAdmissionState::SettledWithoutBody)
        {
            return false;
        }
        entry.readiness = DeferredAdmissionReadiness::SettledWithoutBody;
        drop(parents);
        self.changed.notify_waiters();
        true
    }

    pub(crate) fn remove(&self, parent: &OwnerInvocationId, start: OplogIndex) -> bool {
        let mut parents = self.parents.lock().unwrap();
        let removed = parents
            .get_mut(parent)
            .is_some_and(|entries| entries.remove(&start).is_some());
        if parents.get(parent).is_some_and(BTreeMap::is_empty) {
            parents.remove(parent);
        }
        drop(parents);
        if removed {
            self.changed.notify_waiters();
        }
        removed
    }

    pub(crate) async fn wait_and_register_cohort(
        &self,
        parent: &OwnerInvocationId,
        cohort: DeferredAdmissionCohort,
        starts: &[OplogIndex],
        operations: &OwnerToolOperations,
        lane: &OwnerLane,
    ) -> Result<Option<OwnerLaneWait>, WorkerExecutorError> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) =
                self.try_claim_cohort(parent, cohort, starts.iter().copied(), |ready| {
                    let result_await_parent =
                        matches!(cohort, DeferredAdmissionCohort::ResultAwait(_)).then_some(parent);
                    operations
                        .register_ready_bodies(lane, ready, result_await_parent)
                        .map(|(_, wait)| wait)
                })
            {
                return result;
            }
            changed.await;
        }
    }

    /// Marks one existing GOL-33 causal cohort eligible and runs its registration while parent
    /// closure and no-body cancellation are excluded. A cohort is claimed only when all members
    /// have settled staging and no earlier eligible Start remains staging.
    fn try_claim_cohort<R, E>(
        &self,
        parent: &OwnerInvocationId,
        cohort: DeferredAdmissionCohort,
        starts: impl IntoIterator<Item = OplogIndex>,
        claim: impl FnOnce(&[OplogIndex]) -> Result<R, E>,
    ) -> Option<Result<R, E>>
    where
        R: Default,
    {
        let starts = starts.into_iter().collect::<Vec<_>>();
        let closed_parents = self.closed_parents.lock().unwrap();
        if cohort != DeferredAdmissionCohort::ParentEnd && closed_parents.contains(parent) {
            return Some(Ok(R::default()));
        }
        let mut parents = self.parents.lock().unwrap();
        let Some(entries) = parents.get_mut(parent) else {
            return Some(Ok(R::default()));
        };
        let starts = starts
            .into_iter()
            .filter(|start| {
                entries
                    .get(start)
                    .is_some_and(|entry| entry.cohort.is_none_or(|assigned| assigned == cohort))
            })
            .collect::<Vec<_>>();
        for start in &starts {
            let entry = entries
                .get_mut(start)
                .expect("filtered deferred admission must remain present");
            entry.cohort = Some(cohort);
        }
        let Some(earliest_requested) = starts.iter().min().copied() else {
            return Some(Ok(R::default()));
        };
        if entries.range(..earliest_requested).any(|(_, entry)| {
            entry.cohort.is_some() && entry.readiness == DeferredAdmissionReadiness::Staging
        }) || starts
            .iter()
            .any(|start| entries[start].readiness == DeferredAdmissionReadiness::Staging)
        {
            return None;
        }

        let mut released = starts;
        released.sort_unstable();
        let released = released
            .into_iter()
            .filter(|start| entries[start].readiness == DeferredAdmissionReadiness::Ready)
            .collect::<Vec<_>>();
        let claimed = match claim(&released) {
            Ok(claimed) => claimed,
            Err(error) => return Some(Err(error)),
        };
        entries.retain(|_, entry| entry.cohort != Some(cohort));
        if entries.is_empty() {
            parents.remove(parent);
        }
        drop(parents);
        drop(closed_parents);
        self.changed.notify_waiters();
        Some(Ok(claimed))
    }

    #[cfg(test)]
    fn release_cohort(
        &self,
        parent: &OwnerInvocationId,
        cohort: DeferredAdmissionCohort,
        starts: impl IntoIterator<Item = OplogIndex>,
    ) -> Option<Vec<OplogIndex>> {
        self.try_claim_cohort(parent, cohort, starts, |ready| {
            Ok::<_, std::convert::Infallible>(ready.to_vec())
        })
        .map(Result::unwrap)
    }
}

#[cfg(test)]
mod tests {
    use super::super::attachment::{AttachmentMemory, ToolAttachmentModeMetadata, attachment_pair};
    use super::super::{
        FutureToolInvokeGet, ToolExecution, ToolExecutionState, capable_result_await_cohort,
    };
    use super::*;
    use crate::preview2::golem::tool::host::ByteStreamFailure;
    use crate::services::active_agents::MemoryGrant;
    use golem_common::model::AgentId;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::entity::{
        AgentEntity, EntityActivationPolicy, ExecutableTarget, FilesystemCapability,
        ToolInvocationDescriptor,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::tool::{
        CompiledToolBinding, SecretKeyScope, ToolFilesystemAccess, ToolName, ToolProvisionConfig,
        ToolSource,
    };
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
    use test_r::{test, timeout};

    fn parent() -> OwnerInvocationId {
        OwnerInvocationId::Agent(OplogIndex::from_u64(1))
    }

    fn activation(filesystem: FilesystemCapability) -> Arc<EntityActivation> {
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::try_from(1_u64).unwrap();
        let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
        let tool_name = ToolName::try_from("search").unwrap();
        Arc::new(
            EntityActivation::new(
                ExecutableTarget::new(component_id, component_revision),
                deployment_revision,
                EntityActivationPolicy::Tool {
                    provision: ToolProvisionConfig::default(),
                    binding: Box::new(CompiledToolBinding {
                        deployment_revision,
                        release_id: None,
                        metadata_digest: Default::default(),
                        agent_type_name: golem_common::model::agent::AgentTypeName(
                            "Agent".to_string(),
                        ),
                        tool_name,
                        version: "1".to_string(),
                        metadata_version: "1".to_string(),
                        account_id: golem_common::model::account::AccountId::new(),
                        account_email: golem_common::model::account::AccountEmail::new(
                            "owner@example.com",
                        ),
                        parameters: golem_common::model::json::NormalizedJsonValue::new(
                            serde_json::json!({}),
                        ),
                        secret_keys_readable: SecretKeyScope::All,
                        secret_keys_revealable: SecretKeyScope::All,
                        filesystem_access: match filesystem {
                            FilesystemCapability::Capable => ToolFilesystemAccess::Allowed,
                            FilesystemCapability::Incapable => ToolFilesystemAccess::Unset,
                        },
                        source: ToolSource::Component {
                            component_id,
                            component_revision,
                            component_name: golem_common::model::component::ComponentName(
                                "tools".to_string(),
                            ),
                        },
                    }),
                },
                filesystem,
            )
            .unwrap(),
        )
    }

    fn context() -> OwnerToolOperationContext {
        context_with_filesystem(FilesystemCapability::Incapable)
    }

    fn capable_context() -> OwnerToolOperationContext {
        context_with_filesystem(FilesystemCapability::Capable)
    }

    fn context_with_filesystem(filesystem: FilesystemCapability) -> OwnerToolOperationContext {
        let owner = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        );
        context_for(&owner, filesystem, EntityCallMode::Asynchronous)
    }

    fn context_for(
        owner: &golem_common::model::OwnedAgentId,
        filesystem: FilesystemCapability,
        call_mode: EntityCallMode,
    ) -> OwnerToolOperationContext {
        OwnerToolOperationContext {
            parent: parent(),
            call_mode,
            activation: activation(filesystem),
            calling_principal: Principal::Agent(golem_common::model::agent::AgentPrincipal {
                agent_id: owner.agent_id.clone(),
            }),
            principal: Principal::Agent(golem_common::model::agent::AgentPrincipal {
                agent_id: owner.agent_id.clone(),
            }),
            descriptor: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
                attempt_ordinal: 0,
                command_path: Vec::new(),
                args: Vec::new(),
                has_stdin: false,
                has_stdout: false,
                declares_stdout: false,
            }),
            input: TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
                SchemaValue::Tuple {
                    elements: Vec::new(),
                },
            ),
        }
    }

    fn accept_provisional(
        provisional: ProvisionalOwnerToolOperation,
        start: u64,
    ) -> OwnerToolOperation {
        let owner = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        );
        let invocation_id = EntityInvocationId::new(
            golem_common::model::entity::OwnedAgentEntityId {
                owner,
                entity: provisional.context().activation.entity(),
            },
            OplogIndex::from_u64(start),
        )
        .unwrap();
        provisional.accept(invocation_id).unwrap()
    }

    fn accepted_operation(
        lane: &OwnerLane,
        call_mode: EntityCallMode,
        start: u64,
    ) -> (
        Arc<OwnerToolOperations>,
        OwnerToolOperation,
        EntityInvocationId,
    ) {
        let operations = OwnerToolOperations::new();
        let operation = operations.create(context_for(
            lane.owner_id(),
            FilesystemCapability::Capable,
            call_mode,
        ));
        let invocation_id = EntityInvocationId::new(
            golem_common::model::entity::OwnedAgentEntityId {
                owner: lane.owner_id().clone(),
                entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            },
            OplogIndex::from_u64(start),
        )
        .unwrap();
        let operation = operation.accept(invocation_id.clone()).unwrap();
        (operations, operation, invocation_id)
    }

    fn tool_execution(
        operation: OwnerToolOperation,
        parent: OwnerInvocationId,
        start: u64,
    ) -> Arc<ToolExecution> {
        Arc::new(ToolExecution {
            parent,
            start: OplogIndex::from_u64(start),
            filesystem: FilesystemCapability::Capable,
            operation,
            cancellable: true,
            state: Mutex::new(ToolExecutionState {
                result: None,
                failure: None,
            }),
            changed: Notify::new(),
            get_active: AtomicBool::new(false),
            cancel: tokio_util::sync::CancellationToken::new(),
        })
    }

    #[test]
    #[timeout("30s")]
    async fn dropping_a_provisional_operation_unregisters_it_and_wakes_parent_waiters() {
        let owner = OwnerToolOperations::new();
        let operation = owner.create(context());
        let waiting_owner = owner.clone();
        let waiting =
            tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
        tokio::task::yield_now().await;

        assert_eq!(owner.operation_count(), 1);
        assert!(!waiting.is_finished());
        drop(operation);
        waiting.await.unwrap();
        assert_eq!(owner.operation_count(), 0);
        assert_eq!(owner.operation_removal_count(), 1);
    }

    #[test]
    #[timeout("30s")]
    async fn attachment_admission_rejection_preselects_ordinary_terminal() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(capable_context()), 2);
        let (stdout, _consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, |_| async { None }),
        );
        stdout.write(vec![1, 2, 3]).await.unwrap();
        let stdout = stdout.controller();
        assert!(operation.attach(None, Some(stdout.clone())));

        assert_eq!(
            operation.activate_live_attachment_memory_accounting().await,
            ToolLiveAdmissionOutcome::ResourceExhausted
        );
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::SelectingOrdinary)
        ));
        assert!(!operation.begin_cancel());

        let failing_owner = owner.clone();
        let owner_failure = tokio::spawn(async move {
            failing_owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("concurrent owner failure"),
                ))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!owner_failure.is_finished());

        operation
            .take_live_attachment_admission_rejection()
            .await
            .expect("admission rejection marker");
        operation.complete_rejected_live_attachment_memory_accounting();
        let terminal = Arc::new(SerializableToolOperationTerminal {
            body_execution: golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
            result: Err(
                golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                    "limit".to_string(),
                ),
            ),
        });
        operation.resolve_ordinary(terminal, true).await;
        assert!(owner_failure.await.unwrap());
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::Ordinary { .. })
        ));
        assert_eq!(
            stdout.metadata().terminal,
            Some(super::super::attachment::ToolAttachmentTerminalMetadata::ResourceExhausted)
        );
        operation.settle().await;
    }

    #[test]
    #[timeout("30s")]
    async fn cancelled_multi_attachment_preparation_rolls_back_and_can_retry() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(capable_context()), 2);
        let (stdin, _stdin_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, |bytes| async move {
                Some(MemoryGrant::inert(bytes))
            }),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_reservation = attempts.clone();
        let (stdout, _stdout_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, move |bytes| {
                let attempt = attempts_for_reservation.fetch_add(1, Ordering::AcqRel);
                async move {
                    if attempt == 0 {
                        std::future::pending().await
                    } else {
                        Some(MemoryGrant::inert(bytes))
                    }
                }
            }),
        );
        stdin.write(vec![1]).await.unwrap();
        stdout.write(vec![2]).await.unwrap();
        let stdin_controller = stdin.controller();
        let stdout_controller = stdout.controller();
        assert!(operation.attach(
            Some(stdin_controller.clone()),
            Some(stdout_controller.clone())
        ));

        let preparing_operation = operation.clone();
        let preparation = tokio::spawn(async move {
            preparing_operation
                .activate_live_attachment_memory_accounting()
                .await
        });
        while attempts.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        preparation.abort();
        assert!(preparation.await.unwrap_err().is_cancelled());

        assert_eq!(
            stdin_controller.live_memory_accounting_state(),
            (false, false)
        );
        assert_eq!(
            stdout_controller.live_memory_accounting_state(),
            (false, false)
        );
        stdin.write(vec![3]).await.unwrap();
        stdout.write(vec![4]).await.unwrap();

        assert_eq!(
            operation.activate_live_attachment_memory_accounting().await,
            ToolLiveAdmissionOutcome::Admitted
        );
        assert_eq!(
            stdin_controller.live_memory_accounting_state(),
            (true, false)
        );
        assert_eq!(
            stdout_controller.live_memory_accounting_state(),
            (true, false)
        );

        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        operation.settle().await;
    }

    #[test]
    async fn empty_incomplete_activation_does_not_skip_later_capable_attachments() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(capable_context()), 2);

        assert_eq!(
            operation.activate_live_attachment_memory_accounting().await,
            ToolLiveAdmissionOutcome::Admitted
        );
        assert_eq!(
            *operation.live_admission.lock().await,
            ToolLiveAdmissionState::Historical
        );

        let (stdout, _consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, |_| async { None }),
        );
        stdout.write(vec![1, 2, 3]).await.unwrap();
        let stdout = stdout.controller();
        assert!(operation.attach(None, Some(stdout)));
        assert_eq!(
            operation.activate_live_attachment_memory_accounting().await,
            ToolLiveAdmissionOutcome::ResourceExhausted
        );
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::SelectingOrdinary)
        ));

        operation
            .take_live_attachment_admission_rejection()
            .await
            .expect("admission rejection marker");
        operation.resolve_ordinary(
            Arc::new(SerializableToolOperationTerminal {
                body_execution: golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
                result: Err(
                    golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                        "limit".to_string(),
                    ),
                ),
            }),
            true,
        ).await;
        operation.settle().await;
    }

    #[test]
    #[timeout("30s")]
    async fn owner_failure_during_capable_attachment_activation_rolls_back_the_batch() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(capable_context()), 2);
        let (stdin, _stdin_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, |bytes| async move {
                Some(MemoryGrant::inert(bytes))
            }),
        );
        let reservation_entered = Arc::new(Notify::new());
        let reservation_release = Arc::new(Notify::new());
        let (stdout, _stdout_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, {
                let reservation_entered = reservation_entered.clone();
                let reservation_release = reservation_release.clone();
                move |bytes| {
                    let reservation_entered = reservation_entered.clone();
                    let reservation_release = reservation_release.clone();
                    async move {
                        reservation_entered.notify_one();
                        reservation_release.notified().await;
                        Some(MemoryGrant::inert(bytes))
                    }
                }
            }),
        );
        stdin.write(vec![1]).await.unwrap();
        stdout.write(vec![2]).await.unwrap();
        let stdin_controller = stdin.controller();
        let stdout_controller = stdout.controller();
        assert!(operation.attach(
            Some(stdin_controller.clone()),
            Some(stdout_controller.clone())
        ));

        let activation = tokio::spawn({
            let operation = operation.clone();
            async move { operation.activate_live_attachment_memory_accounting().await }
        });
        reservation_entered.notified().await;
        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed during attachment activation"),
                ))
                .await
        );
        reservation_release.notify_one();

        assert_eq!(activation.await.unwrap(), ToolLiveAdmissionOutcome::Fenced);
        assert_eq!(
            stdin_controller.live_memory_accounting_state(),
            (false, false)
        );
        assert_eq!(
            stdout_controller.live_memory_accounting_state(),
            (false, false)
        );
        assert!(stdin_controller.metadata().owner_fenced);
        assert!(stdout_controller.metadata().owner_fenced);

        owner.drain_owner_failure_lanes().await;
        operation.settle().await;
    }

    #[test]
    #[timeout("30s")]
    async fn cancellation_during_capable_attachment_activation_rolls_back_the_batch() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(capable_context()), 2);
        let (stdin, _stdin_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, |bytes| async move {
                Some(MemoryGrant::inert(bytes))
            }),
        );
        let reservation_entered = Arc::new(Notify::new());
        let reservation_release = Arc::new(Notify::new());
        let (stdout, _stdout_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, {
                let reservation_entered = reservation_entered.clone();
                let reservation_release = reservation_release.clone();
                move |bytes| {
                    let reservation_entered = reservation_entered.clone();
                    let reservation_release = reservation_release.clone();
                    async move {
                        reservation_entered.notify_one();
                        reservation_release.notified().await;
                        Some(MemoryGrant::inert(bytes))
                    }
                }
            }),
        );
        stdin.write(vec![1]).await.unwrap();
        stdout.write(vec![2]).await.unwrap();
        let stdin_controller = stdin.controller();
        let stdout_controller = stdout.controller();
        assert!(operation.attach(
            Some(stdin_controller.clone()),
            Some(stdout_controller.clone())
        ));

        let local_live_tail = Arc::new(AtomicBool::new(false));
        let activation = tokio::spawn({
            let operation = operation.clone();
            let local_live_tail = local_live_tail.clone();
            async move {
                crate::durable_host::PendingReplayToLive {
                    replay_target: OplogIndex::from_u64(9),
                    role: crate::durable_host::replay_state::ReplayToLiveRole::NonPrimary,
                    replaying_incomplete_entity: true,
                    tool_entity: true,
                    tool_operation: Some(operation),
                    local_live_tail,
                }
                .finish()
                .await
            }
        });
        reservation_entered.notified().await;
        assert!(operation.begin_cancel());
        reservation_release.notify_one();

        let outcome = activation.await.unwrap().unwrap();
        assert_eq!(outcome, crate::durable_host::FinishReplayToLive::Cancelled);
        assert!(!local_live_tail.load(Ordering::Acquire));
        let live_action_ran = Arc::new(AtomicBool::new(false));
        if outcome.require_live().is_ok() {
            live_action_ran.store(true, Ordering::Release);
        }
        assert!(!live_action_ran.load(Ordering::Acquire));
        assert_eq!(
            stdin_controller.live_memory_accounting_state(),
            (false, false)
        );
        assert_eq!(
            stdout_controller.live_memory_accounting_state(),
            (false, false)
        );
        operation.resolve_cancel(true).await;
        operation.settle().await;
    }

    #[test]
    #[timeout("30s")]
    async fn cancellation_during_failed_attachment_activation_rolls_back_the_batch() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(capable_context()), 2);
        let (stdin, _stdin_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, |bytes| async move {
                Some(MemoryGrant::inert(bytes))
            }),
        );
        let reservation_entered = Arc::new(Notify::new());
        let reservation_release = Arc::new(Notify::new());
        let (stdout, _stdout_consumer, _) = attachment_pair(
            8,
            AttachmentMemory::with_test_reservation(false, {
                let reservation_entered = reservation_entered.clone();
                let reservation_release = reservation_release.clone();
                move |_| {
                    let reservation_entered = reservation_entered.clone();
                    let reservation_release = reservation_release.clone();
                    async move {
                        reservation_entered.notify_one();
                        reservation_release.notified().await;
                        None
                    }
                }
            }),
        );
        stdin.write(vec![1]).await.unwrap();
        stdout.write(vec![2]).await.unwrap();
        let stdin_controller = stdin.controller();
        let stdout_controller = stdout.controller();
        assert!(operation.attach(
            Some(stdin_controller.clone()),
            Some(stdout_controller.clone())
        ));

        let activation = tokio::spawn({
            let operation = operation.clone();
            async move { operation.activate_live_attachment_memory_accounting().await }
        });
        reservation_entered.notified().await;
        assert!(operation.begin_cancel());
        reservation_release.notify_one();

        assert_eq!(
            activation.await.unwrap(),
            ToolLiveAdmissionOutcome::Cancelled
        );
        assert_eq!(
            stdin_controller.live_memory_accounting_state(),
            (false, false)
        );
        assert_eq!(
            stdout_controller.live_memory_accounting_state(),
            (false, false)
        );
        assert!(owner.owner_winner().is_none());
        operation.resolve_cancel(true).await;
        operation.settle().await;
    }

    #[test]
    fn dropping_an_accepted_observer_does_not_drop_the_owner_operation() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let lease = operation.lease.clone();

        assert_eq!(owner.operation_count(), 1);
        drop(operation);
        assert_eq!(owner.operation_count(), 1);
        assert_eq!(lease.handles.load(Ordering::Acquire), 0);
        assert_eq!(owner.operation_removal_count(), 0);
    }

    #[test]
    #[timeout("30s")]
    async fn owner_failure_drain_removes_a_handleless_operation_and_wakes_parent_waiters() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let lease = operation.lease.clone();
        drop(operation);
        assert_eq!(lease.handles.load(Ordering::Acquire), 0);
        assert_eq!(owner.operation_count(), 1);

        let waiting_owner = owner.clone();
        let waiting =
            tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed"),
                ))
                .await
        );
        owner.drain_owner_failure_lanes().await;

        waiting.await.unwrap();
        assert_eq!(owner.operation_count(), 0);
        assert_eq!(owner.operation_removal_count(), 1);
    }

    #[test]
    #[timeout("30s")]
    async fn concurrent_final_handle_drops_remove_once_and_wake_parent_waiters() {
        let owner = OwnerToolOperations::new();
        let first = accept_provisional(owner.create(context()), 2);
        let second = first.clone();
        let lease = first.lease.clone();
        assert!(first.begin_cancel());
        first.resolve_cancel(true).await;

        let waiting_owner = owner.clone();
        let waiting =
            tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_barrier = barrier.clone();
        let first_drop = tokio::spawn(async move {
            first_barrier.wait().await;
            drop(first);
        });
        let second_barrier = barrier.clone();
        let second_drop = tokio::spawn(async move {
            second_barrier.wait().await;
            drop(second);
        });
        barrier.wait().await;
        first_drop.await.unwrap();
        second_drop.await.unwrap();

        waiting.await.unwrap();
        assert_eq!(lease.handles.load(Ordering::Acquire), 0);
        assert_eq!(owner.operation_count(), 0);
        assert_eq!(owner.operation_removal_count(), 1);
    }

    #[test]
    async fn metadata_exposes_mode_backpressure_and_sanitized_owner_failure() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let (stdout, _reader, _observer) = attachment_pair(4, AttachmentMemory::inert());
        assert!(operation.attach(None, Some(stdout.controller())));
        assert!(stdout.configure_live());
        stdout.write(vec![1, 2, 3, 4]).await.unwrap();

        let metadata = owner.metadata();
        let operation_metadata = &metadata.operations[0];
        assert_eq!(operation_metadata.call_mode, EntityCallMode::Asynchronous);
        assert_eq!(
            operation_metadata.filesystem,
            FilesystemCapability::Incapable
        );
        let stdout_metadata = operation_metadata.stdout.as_ref().unwrap();
        assert_eq!(stdout_metadata.capacity_bytes, 4);
        assert_eq!(stdout_metadata.buffered_bytes, 4);
        assert_eq!(stdout_metadata.charged_bytes, 4);
        assert!(stdout_metadata.backpressured);

        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("private infrastructure detail"),
                ))
                .await
        );
        let metadata = owner.metadata();
        assert_eq!(
            metadata.owner_failure,
            Some(ToolOwnerFailureMetadata::Infrastructure)
        );
        let stdout_metadata = metadata.operations[0].stdout.as_ref().unwrap();
        assert_eq!(
            stdout_metadata.terminal,
            Some(super::super::attachment::ToolAttachmentTerminalMetadata::Cancelled)
        );
        assert!(stdout_metadata.owner_fenced);
        assert!(!stdout_metadata.backpressured);

        owner.drain_owner_failure_lanes().await;
        operation.settle().await;
    }

    #[test]
    async fn attaching_to_a_fenced_operation_fences_and_clears_local_attachments() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let (stdout, consumer, _observer) = attachment_pair(4, AttachmentMemory::inert());
        let controller = consumer.controller();
        stdout.write(vec![1, 2, 3, 4]).await.unwrap();

        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed before attachment"),
                ))
                .await
        );
        assert!(!operation.attach(None, Some(controller.clone())));

        let metadata = controller.metadata();
        assert!(metadata.owner_fenced);
        assert_eq!(metadata.mode, ToolAttachmentModeMetadata::Pending);
        assert_eq!(metadata.buffered_bytes, 0);
        assert_eq!(metadata.charged_bytes, 0);

        owner.drain_owner_failure_lanes().await;
        operation.settle().await;
    }

    #[test]
    async fn accepted_operation_gates_parent_settlement_until_owner_settlement() {
        let owner = OwnerToolOperations::new();
        let provisional = owner.create(context());
        let operation = accept_provisional(provisional, 2);
        let waiting_owner = owner.clone();
        let waiting =
            tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed"),
                ))
                .await
        );
        owner.drain_owner_failure_lanes().await;
        assert!(!waiting.is_finished());
        operation.settle().await;

        waiting.await.unwrap();
        assert_eq!(owner.operation_count(), 0);
        assert_eq!(owner.operation_removal_count(), 1);
    }

    #[test]
    #[timeout("30s")]
    async fn owner_failure_becomes_interruptible_only_after_operation_cleanup() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let sibling = accept_provisional(owner.create(context()), 3);
        let waiting_owner = owner.clone();
        let waiting =
            tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        assert!(
            operation
                .select_infrastructure(WorkerExecutorError::runtime("owner failed"))
                .await
        );
        let cleanup = operation
            .claim_owner_failure_cleanup()
            .expect("the selecting operation owns cleanup");
        assert!(matches!(
            sibling.winner_if_active(),
            Some(ToolOperationWinner::FencedByOwner)
        ));
        owner.drain_owner_failure_lanes().await;
        assert_eq!(owner.operation_count(), 2);
        assert!(owner.interruptible_owner_failure().is_none());

        operation.settle().await;
        sibling.settle().await;
        owner.wait_owner_settled().await;
        owner.complete_owner_failure_cleanup(cleanup);

        waiting.await.unwrap();
        assert_eq!(owner.operation_count(), 0);
        assert_eq!(owner.operation_removal_count(), 2);
        assert!(matches!(
            owner.interruptible_owner_failure(),
            Some(OwnerFailureWinner::Infrastructure(_))
        ));
    }

    #[test]
    #[timeout("30s")]
    async fn owner_failure_seals_operation_creation_through_cleanup_completion() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        assert!(
            operation
                .select_infrastructure(WorkerExecutorError::runtime("owner failed"))
                .await
        );
        let cleanup = operation
            .claim_owner_failure_cleanup()
            .expect("the selecting operation owns cleanup");

        let during_cleanup = owner.create(context());
        assert!(during_cleanup.is_rejected());
        assert_eq!(owner.operation_count(), 1);

        operation.settle().await;
        owner.wait_owner_settled().await;
        owner.complete_owner_failure_cleanup(cleanup);

        let after_cleanup = owner.create(context());
        assert!(after_cleanup.is_rejected());
        assert_eq!(owner.operation_count(), 0);
        assert!(matches!(
            owner.interruptible_owner_failure(),
            Some(OwnerFailureWinner::Infrastructure(_))
        ));
    }

    #[test]
    #[timeout("30s")]
    async fn settlement_wakes_parent_waiters_before_cancellable_lane_drain() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let acquisition = tokio::spawn(std::future::pending::<()>());
        {
            let mut state = owner.state.lock().unwrap();
            state.operations.get_mut(&operation.id).unwrap().lane =
                LaneOwnership::Acquiring(AcquisitionControl {
                    abort: acquisition.abort_handle(),
                    drained: Arc::new(AcquisitionDrain::default()),
                });
        }
        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed"),
                ))
                .await
        );

        let waiting_owner = owner.clone();
        let waiting =
            tokio::spawn(async move { waiting_owner.wait_parent_settled(&parent()).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        let settlement = tokio::spawn(operation.settle());
        while owner.operation_count() != 0 {
            tokio::task::yield_now().await;
        }
        assert!(!settlement.is_finished());
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();

        settlement.abort();
        assert!(settlement.await.unwrap_err().is_cancelled());
        assert!(acquisition.await.unwrap_err().is_cancelled());
        assert_eq!(owner.operation_removal_count(), 1);
    }

    #[test]
    async fn owner_failure_waits_for_non_interleavable_terminal_selection() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        assert!(operation.begin_cancel());

        let selecting_owner = owner.clone();
        let failure = tokio::spawn(async move {
            selecting_owner
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed"),
                ))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!failure.is_finished());

        operation.resolve_cancel(true).await;
        assert!(failure.await.unwrap());
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::Cancelled)
        ));
        assert!(matches!(
            owner.owner_winner(),
            Some(OwnerFailureWinner::Infrastructure(_))
        ));
    }

    #[test]
    async fn explicit_cancel_preserves_owner_interrupt_and_fences_streams_before_return() {
        let owner = OwnerToolOperations::new();
        let cancelled = accept_provisional(owner.create(context()), 2);
        let sibling = accept_provisional(owner.create(context()), 3);
        let (_cancelled_producer, cancelled_consumer, cancelled_observer) =
            attachment_pair(16, AttachmentMemory::inert());
        let (_sibling_producer, sibling_consumer, sibling_observer) =
            attachment_pair(16, AttachmentMemory::inert());
        assert!(cancelled.attach(None, Some(cancelled_consumer.controller())));
        assert!(sibling.attach(None, Some(sibling_consumer.controller())));

        assert!(cancelled.begin_cancel());
        let selecting_owner = owner.clone();
        let lifecycle = tokio::spawn(async move {
            selecting_owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!lifecycle.is_finished());

        cancelled.resolve_cancel(true).await;
        assert!(lifecycle.await.unwrap());

        assert!(matches!(
            cancelled_observer.wait_terminal().await,
            crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
                ByteStreamFailure::Cancelled
            )
        ));
        assert!(matches!(
            sibling_observer.wait_terminal().await,
            crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
                ByteStreamFailure::Cancelled
            )
        ));
        assert!(matches!(
            cancelled.winner_if_active(),
            Some(ToolOperationWinner::Cancelled)
        ));
        assert!(matches!(
            sibling.winner_if_active(),
            Some(ToolOperationWinner::FencedByOwner)
        ));
        assert!(matches!(
            owner.owner_winner(),
            Some(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
        ));
    }

    #[test]
    async fn lifecycle_winner_forces_a_losing_guest_trap_stdout_to_cancelled() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let (_producer, consumer, observer) = attachment_pair(16, AttachmentMemory::inert());
        let stdout = consumer.controller();
        assert!(operation.attach(None, Some(stdout.clone())));

        assert!(
            owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
                .await
        );
        let failure =
            super::super::guest_trap_stdout_failure(&operation, TrapType::Exit, false, false).await;
        assert!(matches!(failure, ByteStreamFailure::Cancelled));
        let _ = stdout.host_fail(failure);
        assert!(matches!(
            owner.owner_winner(),
            Some(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
        ));
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::FencedByOwner)
        ));
        assert!(matches!(
            observer.wait_terminal().await,
            crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
                ByteStreamFailure::Cancelled
            )
        ));
    }

    #[test]
    async fn guest_trap_winner_rejects_a_later_lifecycle_failure() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);

        assert!(operation.select_trap(TrapType::Exit).await);
        assert!(
            !owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
                .await
        );
        assert!(matches!(
            owner.owner_winner(),
            Some(OwnerFailureWinner::Trap(TrapType::Exit))
        ));
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::Trap)
        ));
    }

    #[test]
    async fn cancel_trap_and_lifecycle_race_selects_one_owner_failure() {
        let owner = OwnerToolOperations::new();
        let cancelled = accept_provisional(owner.create(context()), 2);
        let trapped = accept_provisional(owner.create(context()), 3);
        assert!(cancelled.begin_cancel());

        let trapping = tokio::spawn({
            let trapped = trapped.clone();
            async move { trapped.select_trap(TrapType::Exit).await }
        });
        let lifecycle = tokio::spawn({
            let owner = owner.clone();
            async move {
                owner
                    .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!trapping.is_finished());
        assert!(!lifecycle.is_finished());

        cancelled.resolve_cancel(true).await;
        let trap_selected = trapping.await.unwrap();
        let lifecycle_selected = lifecycle.await.unwrap();
        assert_ne!(trap_selected, lifecycle_selected);
        assert!(matches!(
            cancelled.winner_if_active(),
            Some(ToolOperationWinner::Cancelled)
        ));
        match owner.owner_winner() {
            Some(OwnerFailureWinner::Trap(TrapType::Exit)) => assert!(trap_selected),
            Some(OwnerFailureWinner::Lifecycle(InterruptKind::Restart)) => {
                assert!(lifecycle_selected)
            }
            other => panic!("three-way race selected an unexpected owner winner: {other:?}"),
        }
    }

    #[test]
    async fn guest_trap_atomically_fences_siblings_and_preserves_exact_owner_winner() {
        let owner = OwnerToolOperations::new();
        let trapped = accept_provisional(owner.create(context()), 2);
        let sibling = accept_provisional(owner.create(context()), 3);
        let (trapped_stdin, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
        let (trapped_stdout, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
        let (sibling_stdin, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
        let (sibling_stdout, _consumer, _) = attachment_pair(16, AttachmentMemory::inert());
        let trapped_stdin = trapped_stdin.controller();
        let trapped_stdout = trapped_stdout.controller();
        let sibling_stdin = sibling_stdin.controller();
        let sibling_stdout = sibling_stdout.controller();
        assert!(trapped.attach(Some(trapped_stdin.clone()), Some(trapped_stdout.clone())));
        assert!(sibling.attach(Some(sibling_stdin.clone()), Some(sibling_stdout.clone())));
        let trap = TrapType::Exit;

        assert!(trapped.select_trap(trap.clone()).await);
        assert!(matches!(
            trapped.winner_if_active(),
            Some(ToolOperationWinner::Trap)
        ));
        assert!(matches!(
            sibling.winner_if_active(),
            Some(ToolOperationWinner::FencedByOwner)
        ));
        assert!(matches!(
            owner.owner_winner(),
            Some(OwnerFailureWinner::Trap(TrapType::Exit))
        ));
        for attachment in [trapped_stdin, trapped_stdout, sibling_stdin, sibling_stdout] {
            assert!(attachment.metadata().owner_fenced);
        }
    }

    #[test]
    async fn guest_trap_drains_a_sibling_blocked_on_lane_acquisition() {
        let owner_id = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        );
        let lane = OwnerLane::new(owner_id.clone());
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let operations = OwnerToolOperations::new();
        let accept = |start| {
            let provisional = operations.create(context_for(
                &owner_id,
                FilesystemCapability::Capable,
                EntityCallMode::Asynchronous,
            ));
            let invocation_id = EntityInvocationId::new(
                golem_common::model::entity::OwnedAgentEntityId {
                    owner: owner_id.clone(),
                    entity: provisional.context().activation.entity(),
                },
                OplogIndex::from_u64(start),
            )
            .unwrap();
            provisional.accept(invocation_id).unwrap()
        };
        let trapped = accept(2);
        let sibling = accept(3);
        assert!(
            sibling.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(sibling.register_body(&lane).unwrap());
        let acquiring_sibling = sibling.clone();
        let acquiring =
            tokio::spawn(async move { acquiring_sibling.acquire_registered_body().await });
        while sibling.is_acquiring_lane_if_active() != Some(true) {
            tokio::task::yield_now().await;
        }

        assert!(trapped.select_trap(TrapType::Exit).await);
        operations.drain_owner_failure_lanes().await;

        assert!(!acquiring.await.unwrap().unwrap());
        assert_eq!(lane.holder(), Some(parent()));
        trapped.settle().await;
        sibling.settle().await;
        primary.complete();
    }

    #[test]
    async fn later_trap_preserves_committed_sibling_terminals() {
        let owner = OwnerToolOperations::new();
        let trapped = accept_provisional(owner.create(context()), 2);
        let ordinary = accept_provisional(owner.create(context()), 3);
        let cancelled = accept_provisional(owner.create(context()), 4);
        let terminal = Arc::new(SerializableToolOperationTerminal {
            body_execution: golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
            result: Err(
                golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                    "limit".to_string(),
                ),
            ),
        });

        assert!(ordinary.begin_ordinary());
        ordinary.resolve_ordinary(terminal.clone(), true).await;
        assert!(cancelled.begin_cancel());
        cancelled.resolve_cancel(true).await;

        assert!(trapped.select_trap(TrapType::Exit).await);
        assert!(matches!(
            trapped.winner_if_active(),
            Some(ToolOperationWinner::Trap)
        ));
        assert!(matches!(
            ordinary.winner_if_active(),
            Some(ToolOperationWinner::Ordinary {
                _terminal: recorded
            }) if recorded == terminal
        ));
        assert!(matches!(
            cancelled.winner_if_active(),
            Some(ToolOperationWinner::Cancelled)
        ));
    }

    #[test]
    async fn cancellation_before_registration_owns_no_lane_value() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let (_operations, operation, _) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert_eq!(operation.owns_lane_value_if_active(), Some(false));

        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        assert_eq!(operation.owns_lane_value_if_active(), Some(false));
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::Cancelled)
        ));
    }

    #[test]
    async fn cancellation_drops_a_queued_lane_ticket_without_starting_the_body() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let (_operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(operation.register_body(&lane).unwrap());
        assert_eq!(operation.owns_lane_value_if_active(), Some(true));

        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        assert_eq!(operation.owns_lane_value_if_active(), Some(false));
        assert_eq!(lane.holder(), Some(parent()));
        assert!(
            lane.await_invocations(&parent(), [OwnerInvocationId::Entity(invocation_id)])
                .is_err()
        );
        primary.complete();
    }

    #[test]
    async fn cancellation_after_grant_releases_permit_before_the_first_guest_poll() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let (_operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(operation.register_body(&lane).unwrap());
        let wait = lane
            .await_invocations(
                &parent(),
                [OwnerInvocationId::Entity(invocation_id.clone())],
            )
            .unwrap();
        assert!(operation.acquire_registered_body().await.unwrap());
        assert_eq!(operation.owns_lane_value_if_active(), Some(true));
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(invocation_id))
        );

        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        assert_eq!(operation.owns_lane_value_if_active(), Some(false));
        wait.wait().await;
        assert_eq!(lane.holder(), Some(parent()));
        primary.complete();
    }

    #[test]
    async fn capable_terminal_commit_returns_lane_before_completion_publication() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let (_operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        let (stdout, _reader, _observer) = attachment_pair(16, AttachmentMemory::inert());
        let stdout_controller = stdout.controller();
        assert!(operation.attach(None, Some(stdout_controller.clone())));
        assert!(stdout.configure_completion());
        stdout.write(b"staged".to_vec()).await.unwrap();
        stdout.finish().unwrap();

        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(operation.register_body(&lane).unwrap());
        let wait = lane
            .await_invocations(
                &parent(),
                [OwnerInvocationId::Entity(invocation_id.clone())],
            )
            .unwrap();
        assert!(operation.acquire_registered_body().await.unwrap());
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(invocation_id))
        );
        assert_eq!(
            stdout_controller.metadata().mode,
            ToolAttachmentModeMetadata::CompletionStaged
        );
        assert_eq!(stdout_controller.metadata().delivered_bytes, 0);

        let terminal = Arc::new(SerializableToolOperationTerminal {
            body_execution: golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
            result: Err(
                golem_common::model::oplog::payload::types::SerializableToolRpcError::ResourceExhausted(
                    "limit".to_string(),
                ),
            ),
        });
        assert!(operation.begin_ordinary());
        operation.resolve_ordinary(terminal, true).await;
        wait.wait().await;

        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::Ordinary { .. })
        ));
        assert_eq!(lane.holder(), Some(parent()));
        assert_eq!(
            stdout_controller.metadata().mode,
            ToolAttachmentModeMetadata::CompletionStaged
        );
        assert_eq!(stdout_controller.metadata().delivered_bytes, 0);

        operation.settle().await;
        assert!(stdout_controller.publish_completion());
        assert_eq!(
            stdout_controller.metadata().mode,
            ToolAttachmentModeMetadata::CompletionPublished
        );
        primary.complete();
    }

    #[test]
    async fn owner_fence_keeps_granted_lane_until_sidecar_drain() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let (operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(operation.register_body(&lane).unwrap());
        let wait = lane
            .await_invocations(
                &parent(),
                [OwnerInvocationId::Entity(invocation_id.clone())],
            )
            .unwrap();
        assert!(operation.acquire_registered_body().await.unwrap());

        assert!(
            operations
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("owner failed"),
                ))
                .await
        );
        assert_eq!(operation.owns_lane_value_if_active(), Some(true));
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(invocation_id))
        );

        operations.drain_owner_failure_lanes().await;
        assert_eq!(operation.owns_lane_value_if_active(), Some(false));
        wait.wait().await;
        assert_eq!(lane.holder(), Some(parent()));
        drop(operation);
        assert_eq!(operations.operation_count(), 0);
        primary.complete();
    }

    #[test]
    async fn cancellation_drains_a_blocked_in_flight_lane_acquisition() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let (_operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(operation.register_body(&lane).unwrap());

        let acquiring_operation = operation.clone();
        let acquiring =
            tokio::spawn(async move { acquiring_operation.acquire_registered_body().await });
        while operation.is_acquiring_lane_if_active() != Some(true) {
            tokio::task::yield_now().await;
        }

        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        assert!(!acquiring.await.unwrap().unwrap());
        assert_eq!(operation.owns_lane_value_if_active(), Some(false));
        assert!(
            lane.await_invocations(&parent(), [OwnerInvocationId::Entity(invocation_id)])
                .is_err()
        );
        assert_eq!(lane.holder(), Some(parent()));
        primary.complete();
    }

    #[test]
    async fn rolled_back_cancellation_preserves_a_permit_granted_during_selection() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let (_operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        assert!(operation.register_body(&lane).unwrap());

        let acquiring_operation = operation.clone();
        let acquiring =
            tokio::spawn(async move { acquiring_operation.acquire_registered_body().await });
        while operation.is_acquiring_lane_if_active() != Some(true) {
            tokio::task::yield_now().await;
        }
        assert!(operation.begin_cancel());
        let wait = lane
            .await_invocations(
                &parent(),
                [OwnerInvocationId::Entity(invocation_id.clone())],
            )
            .unwrap();
        tokio::task::yield_now().await;
        assert!(!acquiring.is_finished());

        operation.resolve_cancel(false).await;
        assert!(acquiring.await.unwrap().unwrap());
        assert_eq!(operation.owns_lane_value_if_active(), Some(true));
        assert_eq!(
            operation.admission_if_active(),
            Some(BodyAdmissionState::Running)
        );
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(invocation_id))
        );

        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        wait.wait().await;
        assert_eq!(lane.holder(), Some(parent()));
        primary.complete();
    }

    #[test]
    async fn cancellation_selection_is_idempotent_until_its_terminal_commits() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let (_operations, operation, _invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);

        assert!(operation.begin_cancel());
        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        assert!(!operation.begin_cancel());
        operation.clone().settle().await;
        assert!(!operation.begin_cancel());
    }

    #[test]
    async fn cancellation_selection_closes_attached_streams() {
        let owner = OwnerToolOperations::new();
        let operation = accept_provisional(owner.create(context()), 2);
        let (_producer, consumer, observer) = attachment_pair(16, AttachmentMemory::inert());
        assert!(operation.attach(None, Some(consumer.controller())));

        assert!(operation.begin_cancel());

        assert!(matches!(
            observer.wait_terminal().await,
            crate::preview2::golem::tool::host::ByteStreamCloseCause::Failed(
                ByteStreamFailure::Cancelled
            )
        ));
        operation.resolve_cancel(true).await;
    }

    #[test]
    async fn local_cancellation_interruption_does_not_enter_trap_election() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let (operations, operation, _invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);

        assert!(operation.claim_local_cancellation_interruption());
        operation.resolve_cancel(true).await;

        assert!(operations.owner_winner().is_none());
        assert!(matches!(
            operation.winner_if_active(),
            Some(ToolOperationWinner::Cancelled)
        ));
    }

    #[test]
    async fn retained_clone_has_only_optional_state_after_settlement() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let (_operations, operation, _invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        let retained = operation.clone();
        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        operation.settle().await;

        assert_eq!(retained.invocation_id(), None);
        assert_eq!(retained.admission_if_active(), None);
        assert!(retained.winner_if_active().is_none());
        assert!(!retained.cancellation_selected_if_active());
        assert_eq!(retained.owns_lane_value_if_active(), None);
        assert_eq!(retained.is_acquiring_lane_if_active(), None);
        let _ = retained.context();
        let _ = retained.subscribe();
    }

    #[test]
    fn completed_future_observation_is_repeatable_without_an_active_cohort() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let (_operations, operation, _invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        let execution = tool_execution(operation, parent(), 2);
        execution.complete(Ok(Err(
            golem_common::model::oplog::payload::types::SerializableToolRpcError::Cancelled,
        )));

        for _ in 0..2 {
            assert!(matches!(
                execution.get_plan(),
                FutureToolInvokeGet::Ready(response)
                    if matches!(
                        *response,
                        Err(golem_common::model::oplog::payload::types::SerializableToolRpcError::Cancelled)
                    )
            ));
        }
        assert!(capable_result_await_cohort(&[execution.get_plan()], &parent()).is_none());
    }

    #[test]
    fn mixed_completed_and_current_futures_only_cohort_the_active_parent() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        ));
        let (_old_operations, old_operation, _old_invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        let (_current_operations, current_operation, _current_invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 3);
        let old_parent = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let current_parent = OwnerInvocationId::Agent(OplogIndex::from_u64(10));
        let old = tool_execution(old_operation, old_parent, 2);
        let current = tool_execution(current_operation, current_parent.clone(), 3);
        old.complete(Ok(Err(
            golem_common::model::oplog::payload::types::SerializableToolRpcError::Cancelled,
        )));

        let cohort =
            capable_result_await_cohort(&[old.get_plan(), current.get_plan()], &current_parent)
                .unwrap();
        assert_eq!(cohort, vec![OplogIndex::from_u64(3)]);
    }

    #[test]
    async fn result_await_batch_promotes_before_parent_end_in_start_order() {
        let owner_id = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        );
        let lane = OwnerLane::new(owner_id.clone());
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let operations = OwnerToolOperations::new();
        let accept = |start| {
            let provisional = operations.create(context_for(
                &owner_id,
                FilesystemCapability::Capable,
                EntityCallMode::Asynchronous,
            ));
            let invocation_id = EntityInvocationId::new(
                golem_common::model::entity::OwnedAgentEntityId {
                    owner: owner_id.clone(),
                    entity: provisional.context().activation.entity(),
                },
                OplogIndex::from_u64(start),
            )
            .unwrap();
            (
                provisional.accept(invocation_id.clone()).unwrap(),
                invocation_id,
            )
        };
        let (earlier, earlier_id) = accept(2);
        let (later, later_id) = accept(3);
        assert!(later.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready));
        assert!(
            earlier.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );

        let registration_operation = earlier.clone();
        let registration_waiter = registration_operation.wait_until_registered();
        tokio::pin!(registration_waiter);
        assert!(futures::poll!(registration_waiter.as_mut()).is_pending());
        let (registered, wait) = operations
            .register_ready_bodies(
                &lane,
                &[OplogIndex::from_u64(3), OplogIndex::from_u64(2)],
                Some(&parent()),
            )
            .unwrap();
        assert_eq!(
            futures::poll!(registration_waiter.as_mut()),
            std::task::Poll::Ready(true)
        );
        assert_eq!(
            registered,
            vec![
                OwnerInvocationId::Entity(earlier_id.clone()),
                OwnerInvocationId::Entity(later_id.clone()),
            ]
        );
        let wait = wait.expect("result-await registration must return its causal barrier");

        let earlier_for_acquire = earlier.clone();
        let earlier_acquire =
            tokio::spawn(async move { earlier_for_acquire.acquire_registered_body().await });
        let later_for_acquire = later.clone();
        let later_acquire =
            tokio::spawn(async move { later_for_acquire.acquire_registered_body().await });
        assert!(earlier_acquire.await.unwrap().unwrap());
        assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(earlier_id)));
        assert!(!later_acquire.is_finished());

        assert!(earlier.begin_cancel());
        earlier.resolve_cancel(true).await;
        assert!(later_acquire.await.unwrap().unwrap());
        assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(later_id)));
        assert!(later.begin_cancel());
        later.resolve_cancel(true).await;
        wait.wait().await;
        assert_eq!(lane.holder(), Some(parent()));

        earlier.settle().await;
        later.settle().await;
        primary.complete();
    }

    #[test]
    fn generation_reset_clears_closed_parents_but_rejects_staged_admissions() {
        let table = DeferredAdmissionTable::default();
        let closed = parent();
        assert_eq!(table.close_parent_and_snapshot(&closed), Some(Vec::new()));
        table
            .begin_generation()
            .expect("closed parents from the fenced generation can be cleared");
        assert!(!table.closed_parents.lock().unwrap().contains(&closed));

        let staged_parent = parent();
        assert!(table.insert(staged_parent, OplogIndex::from_u64(2)));
        assert!(table.begin_generation().is_err());
    }

    #[test]
    fn deferred_cohort_waits_for_earlier_eligible_staging_and_releases_in_start_order() {
        let table = DeferredAdmissionTable::default();
        let parent = parent();
        let first = OplogIndex::from_u64(2);
        let second = OplogIndex::from_u64(3);
        let third = OplogIndex::from_u64(4);
        assert!(table.insert(parent.clone(), first));
        assert!(table.insert(parent.clone(), second));
        assert!(table.insert(parent.clone(), third));
        assert!(table.settle_staging(&parent, second, DeferredAdmissionReadiness::Ready));
        assert!(table.settle_staging(&parent, third, DeferredAdmissionReadiness::Ready));

        let first_cohort = DeferredAdmissionCohort::ResultAwait(first);
        let second_cohort = DeferredAdmissionCohort::ResultAwait(third);
        assert_eq!(
            table.release_cohort(&parent, first_cohort, [first, second]),
            None
        );
        assert_eq!(table.release_cohort(&parent, second_cohort, [third]), None);
        assert!(table.settle_staging(&parent, first, DeferredAdmissionReadiness::Ready));
        assert_eq!(
            table.release_cohort(&parent, first_cohort, [first, second]),
            Some(vec![first, second])
        );
        assert_eq!(
            table.release_cohort(&parent, second_cohort, [third]),
            Some(vec![third])
        );
    }

    #[test]
    fn no_body_members_settle_a_cohort_without_lane_registration() {
        let table = DeferredAdmissionTable::default();
        let parent = parent();
        let skipped = OplogIndex::from_u64(2);
        let ready = OplogIndex::from_u64(3);
        assert!(table.insert(parent.clone(), skipped));
        assert!(table.insert(parent.clone(), ready));
        assert!(table.settle_staging(
            &parent,
            skipped,
            DeferredAdmissionReadiness::SettledWithoutBody
        ));
        assert!(table.settle_staging(&parent, ready, DeferredAdmissionReadiness::Ready));

        assert_eq!(
            table.release_cohort(
                &parent,
                DeferredAdmissionCohort::ResultAwait(ready),
                [ready, skipped]
            ),
            Some(vec![ready])
        );
    }

    #[test]
    async fn result_await_skips_a_member_released_after_planning_without_losing_survivors() {
        let table = Arc::new(DeferredAdmissionTable::default());
        let parent = parent();
        let released = OplogIndex::from_u64(2);
        let survivor = OplogIndex::from_u64(3);
        assert!(table.insert(parent.clone(), released));
        assert!(table.insert(parent.clone(), survivor));
        assert!(table.settle_staging(&parent, released, DeferredAdmissionReadiness::Ready));
        assert!(table.settle_staging(&parent, survivor, DeferredAdmissionReadiness::Ready));

        let release_barrier = Arc::new(tokio::sync::Barrier::new(2));
        let cohort_table = table.clone();
        let cohort_parent = parent.clone();
        let cohort_barrier = release_barrier.clone();
        let cohort = tokio::spawn(async move {
            cohort_barrier.wait().await;
            cohort_table.release_cohort(
                &cohort_parent,
                DeferredAdmissionCohort::ResultAwait(released),
                [released, survivor],
            )
        });

        assert!(table.remove(&parent, released));
        release_barrier.wait().await;
        assert_eq!(cohort.await.unwrap(), Some(vec![survivor]));
    }

    #[test]
    fn cohort_claim_holds_parent_close_lock_through_lane_registration() {
        let owner_id = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        );
        let lane = OwnerLane::new(owner_id);
        let _primary = lane.enter_primary(OplogIndex::from_u64(1)).unwrap();
        let (operations, operation, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            operation.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        let table = Arc::new(DeferredAdmissionTable::default());
        let parent = parent();
        let start = OplogIndex::from_u64(2);
        assert!(table.insert(parent.clone(), start));
        assert!(table.settle_staging(&parent, start, DeferredAdmissionReadiness::Ready));

        let (claim_entered_tx, claim_entered_rx) = std::sync::mpsc::channel();
        let (continue_claim_tx, continue_claim_rx) = std::sync::mpsc::channel();
        let claim_table = table.clone();
        let claim_operations = operations.clone();
        let claim_lane = lane.clone();
        let claim_parent = parent.clone();
        let claim = std::thread::spawn(move || {
            claim_table
                .try_claim_cohort(
                    &claim_parent,
                    DeferredAdmissionCohort::ResultAwait(start),
                    [start],
                    |ready| {
                        claim_entered_tx.send(()).unwrap();
                        continue_claim_rx.recv().unwrap();
                        let (invocations, _wait) = claim_operations.register_ready_bodies(
                            &claim_lane,
                            ready,
                            Some(&claim_parent),
                        )?;
                        Ok::<_, WorkerExecutorError>(invocations)
                    },
                )
                .unwrap()
                .unwrap()
        });

        claim_entered_rx.recv().unwrap();
        assert!(table.closed_parents.try_lock().is_err());
        continue_claim_tx.send(()).unwrap();
        assert_eq!(
            claim.join().unwrap(),
            vec![OwnerInvocationId::Entity(invocation_id)]
        );
        assert_eq!(
            operation.admission_if_active(),
            Some(BodyAdmissionState::Registered)
        );
        assert_eq!(table.close_parent_and_snapshot(&parent), Some(Vec::new()));
    }

    #[test]
    fn ready_cancellation_and_lane_registration_have_one_winner() {
        let owner_id = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "Agent(owner)".to_string(),
            },
        );
        let lane = OwnerLane::new(owner_id);
        let _primary = lane.enter_primary(OplogIndex::from_u64(1)).unwrap();
        let parent = parent();
        let start = OplogIndex::from_u64(2);

        let (cancelled_operations, cancelled, _) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 2);
        assert!(
            cancelled.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        let cancelled_table = DeferredAdmissionTable::default();
        assert!(cancelled_table.insert(parent.clone(), start));
        assert!(cancelled_table.settle_staging(&parent, start, DeferredAdmissionReadiness::Ready));
        assert!(cancelled_table.settle_operation_without_body(
            &parent,
            start,
            DeferredAdmissionReadiness::Ready,
            &cancelled,
            BodyAdmissionState::Ready,
        ));
        assert_eq!(
            cancelled.admission_if_active(),
            Some(BodyAdmissionState::SettledWithoutBody)
        );
        assert_eq!(
            cancelled_table.release_cohort(
                &parent,
                DeferredAdmissionCohort::ResultAwait(start),
                [start]
            ),
            Some(Vec::new())
        );
        drop(cancelled_operations);

        let (registered_operations, registered, invocation_id) =
            accepted_operation(&lane, EntityCallMode::Asynchronous, 3);
        let registered_start = OplogIndex::from_u64(3);
        assert!(
            registered.transition_admission(BodyAdmissionState::Staging, BodyAdmissionState::Ready)
        );
        let registered_table = DeferredAdmissionTable::default();
        assert!(registered_table.insert(parent.clone(), registered_start));
        assert!(registered_table.settle_staging(
            &parent,
            registered_start,
            DeferredAdmissionReadiness::Ready
        ));
        assert_eq!(
            registered_table
                .try_claim_cohort(
                    &parent,
                    DeferredAdmissionCohort::ResultAwait(registered_start),
                    [registered_start],
                    |ready| {
                        let (invocations, _wait) = registered_operations.register_ready_bodies(
                            &lane,
                            ready,
                            Some(&parent),
                        )?;
                        Ok::<_, WorkerExecutorError>(invocations)
                    },
                )
                .unwrap()
                .unwrap(),
            vec![OwnerInvocationId::Entity(invocation_id)]
        );
        assert!(!registered_table.settle_operation_without_body(
            &parent,
            registered_start,
            DeferredAdmissionReadiness::Ready,
            &registered,
            BodyAdmissionState::Ready,
        ));
        assert_eq!(
            registered.admission_if_active(),
            Some(BodyAdmissionState::Registered)
        );
    }

    #[test]
    fn parent_end_closes_admission_and_consumes_the_atomic_snapshot() {
        let table = DeferredAdmissionTable::default();
        let parent = parent();
        let ready = OplogIndex::from_u64(2);
        let skipped = OplogIndex::from_u64(3);
        assert!(table.insert(parent.clone(), ready));
        assert!(table.insert(parent.clone(), skipped));
        assert!(table.settle_staging(&parent, ready, DeferredAdmissionReadiness::Ready));
        assert!(table.settle_staging(
            &parent,
            skipped,
            DeferredAdmissionReadiness::SettledWithoutBody
        ));

        assert_eq!(
            table.close_parent_and_snapshot(&parent),
            Some(vec![ready, skipped])
        );
        assert!(!table.insert(parent.clone(), OplogIndex::from_u64(4)));
        assert_eq!(
            table.release_cohort(
                &parent,
                DeferredAdmissionCohort::ParentEnd,
                [ready, skipped]
            ),
            Some(vec![ready])
        );
        assert!(table.clear_closed_parent(&parent));
    }

    #[test]
    fn parent_end_supersedes_an_unfinished_result_await_cohort() {
        let table = DeferredAdmissionTable::default();
        let parent = parent();
        let first = OplogIndex::from_u64(2);
        let second = OplogIndex::from_u64(3);
        assert!(table.insert(parent.clone(), first));
        assert!(table.insert(parent.clone(), second));
        assert_eq!(
            table.release_cohort(
                &parent,
                DeferredAdmissionCohort::ResultAwait(second),
                [second]
            ),
            None
        );
        assert_eq!(
            table.close_parent_and_snapshot(&parent),
            Some(vec![first, second])
        );
        assert!(table.settle_staging(&parent, first, DeferredAdmissionReadiness::Ready));
        assert!(table.settle_staging(&parent, second, DeferredAdmissionReadiness::Ready));
        assert_eq!(
            table.release_cohort(
                &parent,
                DeferredAdmissionCohort::ResultAwait(second),
                [second]
            ),
            Some(Vec::new())
        );
        assert_eq!(
            table.release_cohort(&parent, DeferredAdmissionCohort::ParentEnd, [first, second]),
            Some(vec![first, second])
        );
        assert!(table.clear_closed_parent(&parent));
    }
}
