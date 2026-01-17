// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

pub use crate::base_model::oplog::{
    public_oplog_entry, OplogEntry, PublicOplogEntry, PublicOplogEntryWithIndex,
};
pub use crate::base_model::OplogIndex;
pub use payload::*;
pub use public_types::*;
pub use raw_types::*;

use crate::model::component::ComponentRevision;

impl OplogEntry {
    pub fn is_end_atomic_region(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndAtomicRegion { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_write(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndRemoteWrite { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_write_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        matches!(self, OplogEntry::EndRemoteWrite { begin_index, .. } if *begin_index == idx)
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

    pub fn is_pre_remote_transaction_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        self.is_pre_commit_remote_transaction(idx) || self.is_pre_rollback_remote_transaction(idx)
    }

    pub fn is_committed_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::CommittedRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_committed_remote_transaction_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        matches!(self, OplogEntry::CommittedRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_rolled_back_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::RolledBackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_rolled_back_remote_transaction_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        matches!(self, OplogEntry::RolledBackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_transaction(&self, idx: OplogIndex) -> bool {
        self.is_committed_remote_transaction(idx) || self.is_rolled_back_remote_transaction(idx)
    }

    pub fn is_end_remote_transaction_s<S>(&self, idx: OplogIndex, s: &S) -> bool {
        self.is_committed_remote_transaction_s(idx, s)
            || self.is_rolled_back_remote_transaction_s(idx, s)
    }

    /// Checks that an "intermediate oplog entry" between a `BeginRemoteWrite` and an `EndRemoteWrite`
    /// is not a RemoteWrite entry which does not belong to the batched remote write started at `idx`.
    /// Side effects in a PersistenceLevel::PersistNothing region are ignored.
    pub fn no_concurrent_side_effect(
        &self,
        idx: OplogIndex,
        persistence_level: &PersistenceLevel,
    ) -> bool {
        if persistence_level == &PersistenceLevel::PersistNothing {
            true
        } else {
            match self {
                OplogEntry::ImportedFunctionInvoked {
                    durable_function_type,
                    ..
                } => match durable_function_type {
                    DurableFunctionType::WriteRemoteBatched(Some(begin_index))
                        if *begin_index == idx =>
                    {
                        true
                    }
                    DurableFunctionType::WriteRemoteTransaction(Some(begin_index))
                        if *begin_index == idx =>
                    {
                        true
                    }
                    DurableFunctionType::ReadLocal => true,
                    DurableFunctionType::WriteLocal => true,
                    DurableFunctionType::ReadRemote => true,
                    _ => false,
                },
                OplogEntry::ExportedFunctionCompleted { .. } => false,
                _ => true,
            }
        }
    }

    pub fn track_persistence_level(
        &self,
        _idx: OplogIndex,
        persistence_level: &mut PersistenceLevel,
    ) {
        if let OplogEntry::ChangePersistenceLevel { level, .. } = self {
            *persistence_level = *level
        }
    }

    pub fn specifies_component_revision(&self) -> Option<ComponentRevision> {
        match self {
            OplogEntry::Create {
                component_revision, ..
            } => Some(*component_revision),
            OplogEntry::SuccessfulUpdate {
                target_revision, ..
            } => Some(*target_revision),
            _ => None,
        }
    }
}
