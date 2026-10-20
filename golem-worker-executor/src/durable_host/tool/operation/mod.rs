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

mod admission;
mod deferred;
mod lane;
mod owner;
mod terminal;

use lane::drain_lane_ownerships;

use super::attachment::{AttachmentController, ToolAttachmentMetadata};
use crate::model::TrapType;
use crate::worker::owner_lane::{
    OwnerInvocationId, OwnerInvocationPermit, OwnerInvocationTicket, OwnerLane, OwnerLaneWait,
};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::agent::Principal;
use golem_common::model::entity::{
    EntityActivation, EntityCallMode, EntityInvocationDescriptor, EntityInvocationId,
    FilesystemCapability,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::payload::types::{
    SerializableToolAdmissionRefusal, SerializableToolOperationTerminal,
};
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
    ResourceExhaustedPending { preselected: bool },
    ResourceExhaustedHandled,
}

pub(crate) struct ToolAttachmentAdmissionRejection {
    pub(crate) preselected: bool,
}

#[derive(Default)]
struct AdmissionRefusalState {
    expected: Option<(SerializableToolAdmissionRefusal, OplogIndex)>,
    observed: Option<SerializableToolAdmissionRefusal>,
    historical_observed: bool,
}

struct LiveAttachmentPreparationRollback {
    attachments: Vec<AttachmentController>,
    armed: bool,
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
    pub stderr: Option<ToolAttachmentMetadata>,
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

#[derive(Clone)]
pub(crate) struct OwnerToolOperationContext {
    pub parent: OwnerInvocationId,
    pub call_mode: EntityCallMode,
    pub activation: Arc<EntityActivation>,
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
    stderr: Option<AttachmentController>,
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

struct AcquisitionDrainGuard {
    drained: Arc<AcquisitionDrain>,
    permit: Option<OwnerInvocationPermit>,
}

struct OwnerToolOperationsState {
    admission_closed: bool,
    owner_winner: Option<OwnerFailureWinner>,
    owner_failure_cleanup: Option<OwnerFailureCleanupState>,
    owner_failure_cleanup_complete: bool,
    owner_failure_cleanup_settled: bool,
    cleanup_task: Option<Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>>,
    operations: HashMap<u64, RegisteredOperation>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnerFailureCleanupOwner {
    Primary,
    Tool(u64),
}

struct OwnerFailureCleanupState {
    owner: OwnerFailureCleanupOwner,
    claimed: bool,
}

pub(crate) struct OwnerFailureCleanupToken {
    owner: OwnerFailureCleanupOwner,
}

/// One arbitration domain for all accepted tool operations in an owner generation. Durable
/// terminal selection uses a two-step selecting state: an owner failure waits for that selection
/// to resolve, so an operation winner and owner winner cannot be chosen independently.
pub(crate) struct OwnerToolOperations {
    next_id: AtomicU64,
    state: Mutex<OwnerToolOperationsState>,
    changed: Notify,
    failure_continuations: tokio_util::task::TaskTracker,
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

pub(super) type CancellationEffect =
    dyn Fn(&OwnerToolOperation, &AttachmentController) + Send + Sync;

pub(crate) struct OwnerToolOperation {
    id: u64,
    context: Arc<OwnerToolOperationContext>,
    owner: Arc<OwnerToolOperations>,
    _winner: watch::Sender<ToolOperationWinner>,
    lease: Arc<OperationLease>,
    live_admission: Arc<tokio::sync::Mutex<ToolLiveAdmissionState>>,
    admission_refusal: Arc<Mutex<AdmissionRefusalState>>,
    cancellation_effect: Arc<std::sync::OnceLock<Box<CancellationEffect>>>,
}

impl OwnerToolOperation {
    pub(crate) fn context(&self) -> &OwnerToolOperationContext {
        &self.context
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

impl DeferredAdmissionTable {
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
pub(crate) mod tests;
