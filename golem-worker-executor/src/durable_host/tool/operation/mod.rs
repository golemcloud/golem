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
mod tests;
