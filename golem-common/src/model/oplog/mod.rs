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

pub mod matcher;
pub mod payload;
mod protobuf;
mod public_types;
pub(crate) mod raw_types;

#[cfg(test)]
mod tests;

pub use crate::base_model::OplogIndex;
pub use crate::base_model::oplog::{
    OplogEntry, PublicOplogEntry, PublicOplogEntryWithIndex, attachment, public_oplog_entry,
};
pub use payload::*;
pub use public_types::*;
pub use raw_types::*;

use crate::model::component::ComponentRevision;

/// The Store an oplog entry was recorded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityAttribution {
    /// The entry kind does not record which Store appended it.
    Unattributed,
    /// Appended by the primary agent Store.
    Agent,
    /// Appended by the entity body whose entity invocation `Start` is at this index.
    EntityBody(OplogIndex),
}

impl EntityAttribution {
    /// The attribution of an entry recorded with the given `entity_parent_start_index` field.
    pub fn of_store(entity_parent_start_index: Option<OplogIndex>) -> Self {
        match entity_parent_start_index {
            Some(start) => Self::EntityBody(start),
            None => Self::Agent,
        }
    }
}

impl OplogEntry {
    pub fn entity_parent_start_index(&self) -> Option<OplogIndex> {
        match self.entity_attribution() {
            EntityAttribution::EntityBody(start) => Some(start),
            EntityAttribution::Agent | EntityAttribution::Unattributed => None,
        }
    }

    /// The Store an entry was recorded from.
    pub fn entity_attribution(&self) -> EntityAttribution {
        match self {
            OplogEntry::Error {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::Jump {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CreateResource {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::DropResource {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::SetRetryPolicy {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::RemoveRetryPolicy {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardEventQueued {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardInstalled {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardInstallFailed {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardRevoked {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardExpired {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardDerived {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferStarted {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferred {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardRevokedCascade {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferConfirmed {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamRegistered {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamItems {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamEnd {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamCancel {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamSession {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::ToolAttachmentRead {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::ToolAttachmentControl {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::SpanClosed {
                entity_parent_start_index,
                ..
            } => EntityAttribution::of_store(*entity_parent_start_index),
            OplogEntry::Create { .. }
            | OplogEntry::Start { .. }
            | OplogEntry::End { .. }
            | OplogEntry::Cancelled { .. }
            | OplogEntry::CompletionDiscarded { .. }
            | OplogEntry::CompletionDelivered { .. }
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::RecoverySucceeded { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::PendingAgentInvocation { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
            | OplogEntry::GrowMemory { .. }
            | OplogEntry::Log { .. }
            | OplogEntry::Restart { .. }
            | OplogEntry::Resumed { .. }
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::BeginRemoteTransaction { .. }
            | OplogEntry::PreCommitRemoteTransaction { .. }
            | OplogEntry::PreRollbackRemoteTransaction { .. }
            | OplogEntry::CommittedRemoteTransaction { .. }
            | OplogEntry::RolledBackRemoteTransaction { .. }
            | OplogEntry::Snapshot { .. }
            | OplogEntry::SnapshotConfirmed { .. }
            | OplogEntry::OplogProcessorCheckpoint { .. }
            | OplogEntry::HostStreamFrame { .. } => EntityAttribution::Unattributed,
        }
    }

    /// True if `self` is the scope-`End` that closes the scope-`Start` at `idx`.
    ///
    /// Scope `End` entries reference their opening `Start` via `start_index`, so this is
    /// a precise match. Nesting and interleaving are handled correctly because only the `End`
    /// closing the `Start` at `idx` matches.
    pub fn is_end_remote_write(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::End { start_index, .. } if *start_index == idx)
    }

    pub fn is_pre_commit_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::PreCommitRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_pre_rollback_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::PreRollbackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_pre_remote_transaction(&self, idx: OplogIndex) -> bool {
        self.is_pre_commit_remote_transaction(idx) || self.is_pre_rollback_remote_transaction(idx)
    }

    pub fn is_committed_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::CommittedRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_rolled_back_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::RolledBackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_transaction(&self, idx: OplogIndex) -> bool {
        self.is_committed_remote_transaction(idx) || self.is_rolled_back_remote_transaction(idx)
    }

    pub fn specifies_component_revision(&self) -> Option<ComponentRevision> {
        match self {
            OplogEntry::Create { parameters, .. } => Some(parameters.component_revision),
            OplogEntry::SuccessfulUpdate {
                target_revision, ..
            } => Some(*target_revision),
            // Keep this exhaustive so a new entry cannot silently fail to update the revision used
            // to interpret the entries that follow it.
            OplogEntry::Start { .. }
            | OplogEntry::End { .. }
            | OplogEntry::Cancelled { .. }
            | OplogEntry::CompletionDiscarded { .. }
            | OplogEntry::CompletionDelivered { .. }
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::Error { .. }
            | OplogEntry::RecoverySucceeded { .. }
            | OplogEntry::Jump { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::PendingAgentInvocation { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
            | OplogEntry::GrowMemory { .. }
            | OplogEntry::CreateResource { .. }
            | OplogEntry::DropResource { .. }
            | OplogEntry::Log { .. }
            | OplogEntry::Restart { .. }
            | OplogEntry::Resumed { .. }
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::BeginRemoteTransaction { .. }
            | OplogEntry::PreCommitRemoteTransaction { .. }
            | OplogEntry::PreRollbackRemoteTransaction { .. }
            | OplogEntry::CommittedRemoteTransaction { .. }
            | OplogEntry::RolledBackRemoteTransaction { .. }
            | OplogEntry::Snapshot { .. }
            | OplogEntry::SnapshotConfirmed { .. }
            | OplogEntry::OplogProcessorCheckpoint { .. }
            | OplogEntry::SetRetryPolicy { .. }
            | OplogEntry::RemoveRetryPolicy { .. }
            | OplogEntry::CardEventQueued { .. }
            | OplogEntry::CardInstalled { .. }
            | OplogEntry::CardInstallFailed { .. }
            | OplogEntry::CardRevoked { .. }
            | OplogEntry::CardExpired { .. }
            | OplogEntry::CardDerived { .. }
            | OplogEntry::CardTransferStarted { .. }
            | OplogEntry::CardTransferred { .. }
            | OplogEntry::CardRevokedCascade { .. }
            | OplogEntry::CardTransferConfirmed { .. }
            | OplogEntry::HostStreamFrame { .. }
            | OplogEntry::StreamRegistered { .. }
            | OplogEntry::StreamItems { .. }
            | OplogEntry::StreamEnd { .. }
            | OplogEntry::StreamCancel { .. }
            | OplogEntry::StreamSession { .. }
            | OplogEntry::ToolAttachmentControl { .. }
            | OplogEntry::ToolAttachmentRead { .. }
            | OplogEntry::SpanClosed { .. } => None,
        }
    }
}

/// Tracks the transitive durable-call tree rooted at one `Start` for owner-oplog projections.
#[derive(Debug, Clone)]
pub struct OplogScopeProjection {
    root: OplogIndex,
    starts: std::collections::HashSet<OplogIndex>,
}

impl OplogScopeProjection {
    pub fn new(root: OplogIndex) -> Self {
        Self {
            root,
            starts: std::collections::HashSet::from([root]),
        }
    }

    /// Returns whether `entry` belongs to the rooted call tree and records descendant Starts for
    /// subsequent entries. Transaction markers reference their owning scope Start explicitly.
    pub fn includes(&mut self, index: OplogIndex, entry: &OplogEntry) -> bool {
        match entry {
            OplogEntry::Start {
                parent_start_index, ..
            } => {
                if index == self.root {
                    true
                } else if parent_start_index.is_some_and(|parent| self.starts.contains(&parent)) {
                    self.starts.insert(index);
                    true
                } else {
                    false
                }
            }
            OplogEntry::End { start_index, .. }
            | OplogEntry::Cancelled { start_index, .. }
            | OplogEntry::CompletionDiscarded { start_index, .. }
            | OplogEntry::CompletionDelivered { start_index, .. } => {
                self.starts.contains(start_index)
            }
            OplogEntry::HostStreamFrame {
                parent_start_index, ..
            }
            | OplogEntry::Log {
                parent_start_index: Some(parent_start_index),
                ..
            } => self.starts.contains(parent_start_index),
            OplogEntry::Error { .. }
            | OplogEntry::Jump { .. }
            | OplogEntry::CreateResource { .. }
            | OplogEntry::DropResource { .. }
            | OplogEntry::SetRetryPolicy { .. }
            | OplogEntry::RemoveRetryPolicy { .. }
            | OplogEntry::CardEventQueued { .. }
            | OplogEntry::CardInstalled { .. }
            | OplogEntry::CardInstallFailed { .. }
            | OplogEntry::CardRevoked { .. }
            | OplogEntry::CardExpired { .. }
            | OplogEntry::CardDerived { .. }
            | OplogEntry::CardTransferStarted { .. }
            | OplogEntry::CardTransferred { .. }
            | OplogEntry::CardRevokedCascade { .. }
            | OplogEntry::CardTransferConfirmed { .. }
            | OplogEntry::StreamRegistered { .. }
            | OplogEntry::StreamItems { .. }
            | OplogEntry::StreamEnd { .. }
            | OplogEntry::StreamCancel { .. }
            | OplogEntry::StreamSession { .. }
            | OplogEntry::ToolAttachmentControl { .. }
            | OplogEntry::ToolAttachmentRead { .. }
            | OplogEntry::SpanClosed { .. } => entry
                .entity_parent_start_index()
                .is_some_and(|parent| self.starts.contains(&parent)),
            OplogEntry::BeginRemoteTransaction {
                original_begin_index: begin,
                ..
            } => self.starts.contains(begin),
            OplogEntry::PreCommitRemoteTransaction { begin_index, .. }
            | OplogEntry::PreRollbackRemoteTransaction { begin_index, .. }
            | OplogEntry::CommittedRemoteTransaction { begin_index, .. }
            | OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => {
                self.starts.contains(begin_index)
            }
            // Keep exclusions explicit so adding an oplog variant requires deciding whether and
            // how it belongs to a durable invocation scope.
            OplogEntry::Create { .. }
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::RecoverySucceeded { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::PendingAgentInvocation { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
            | OplogEntry::GrowMemory { .. }
            | OplogEntry::Log {
                parent_start_index: None,
                ..
            }
            | OplogEntry::Restart { .. }
            | OplogEntry::Resumed { .. }
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::Snapshot { .. }
            | OplogEntry::SnapshotConfirmed { .. }
            | OplogEntry::OplogProcessorCheckpoint { .. } => false,
        }
    }
}
