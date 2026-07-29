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
    OplogEntry, PublicOplogEntry, PublicOplogEntryWithIndex, public_oplog_entry,
};
pub use payload::*;
pub use public_types::*;
pub use raw_types::*;

use crate::model::component::ComponentRevision;

impl OplogEntry {
    pub fn is_end_atomic_region(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndAtomicRegion { begin_index, .. } if *begin_index == idx)
    }

    /// True if `self` is the scope-`End` that closes the scope-`Start` at `idx`.
    ///
    /// Scope `End` entries reference their opening `Start` via `start_index`, so this is
    /// a precise match (the exact equivalent of the old `EndRemoteWrite { begin_index }`
    /// pairing). Nesting and interleaving are handled correctly because only the `End`
    /// closing the `Start` at `idx` matches.
    pub fn is_end_remote_write(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::End { start_index, .. } if *start_index == idx)
    }

    pub fn is_end_remote_write_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        self.is_end_remote_write(idx)
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

    /// Checks that an "intermediate oplog entry" between a scope `Start` and its matching `End`
    /// does not represent a *foreign* side effect — one that does not belong to the scope rooted at
    /// `state.root`. Membership is determined by `parent_start_index` nesting, tracked transitively
    /// in [`ScopeScanState`] (a grandchild's parent is an inner scope, not the root), so it must be
    /// used together with [`OplogEntry::track_scope_membership`] as the scan's state updater.
    ///
    /// Foreign benign operations (`ReadLocal`, `WriteLocal`, `ReadRemote`) are allowed to interleave;
    /// foreign external writes and foreign batched/transaction scopes are rejected. Side effects in a
    /// `PersistenceLevel::PersistNothing` region are ignored.
    pub fn no_concurrent_side_effect(
        &self,
        _begin_idx: OplogIndex,
        state: &ScopeScanState,
    ) -> bool {
        if state.persistence_level == PersistenceLevel::PersistNothing {
            true
        } else {
            match self {
                OplogEntry::Start {
                    durable_function_type,
                    ..
                } => {
                    if state.current_is_descendant_scope {
                        // A (transitive) descendant scope of the root — part of this scope.
                        true
                    } else {
                        // A foreign scope: only benign reads/local writes may interleave.
                        matches!(
                            durable_function_type,
                            DurableFunctionType::ReadLocal
                                | DurableFunctionType::WriteLocal
                                | DurableFunctionType::ReadRemote
                        )
                    }
                }
                // `End` entries are pure markers and do not themselves cause a side effect.
                OplogEntry::End { .. } => true,
                OplogEntry::Cancelled { .. } => true,
                // A delayed terminal for an asynchronous host operation can be
                // appended after the guest invocation result has already been
                // recorded (for example a dropped P3 HTTP response body whose
                // cleanup is driven by resource destruction). The invocation
                // result marker is not itself a remote side effect, so it must
                // not make the owning durable scope unreplayable.
                OplogEntry::AgentInvocationFinished { .. } => true,
                _ => true,
            }
        }
    }

    /// State updater paired with [`OplogEntry::no_concurrent_side_effect`]. For each scanned entry
    /// (with its own `idx`) it tracks the active persistence level and grows the set of transitive
    /// descendant scopes of `state.root`: a `Start` whose `parent_start_index` is the root or any
    /// already-known descendant becomes a descendant itself. `current_is_descendant_scope` records
    /// the decision for the entry just processed, so the immediately following
    /// `no_concurrent_side_effect` check can use it.
    pub fn track_scope_membership(&self, idx: OplogIndex, state: &mut ScopeScanState) {
        if let OplogEntry::ChangePersistenceLevel {
            persistence_level: level,
            ..
        } = self
        {
            state.persistence_level = *level;
        }

        state.current_is_descendant_scope = false;
        if let OplogEntry::Start {
            parent_start_index: Some(parent),
            ..
        } = self
            && (*parent == state.root || state.descendants.contains(parent))
        {
            state.descendants.insert(idx);
            state.current_is_descendant_scope = true;
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

/// Mutable state carried by a forward scan that validates a durable scope rooted at `root` has no
/// foreign concurrent side effects (see [`OplogEntry::no_concurrent_side_effect`] and
/// [`OplogEntry::track_scope_membership`]).
#[derive(Debug, Clone)]
pub struct ScopeScanState {
    /// The `Start` index of the scope being validated.
    pub root: OplogIndex,
    /// The persistence level active at the current scan position.
    pub persistence_level: PersistenceLevel,
    /// All transitive descendant scope `Start` indices seen so far.
    pub descendants: std::collections::HashSet<OplogIndex>,
    /// Whether the entry processed most recently by `track_scope_membership` was a descendant scope
    /// `Start`. Read by the immediately following `no_concurrent_side_effect` check.
    pub current_is_descendant_scope: bool,
}

impl ScopeScanState {
    /// Creates a fresh scan state for the scope rooted at `root`, starting from `persistence_level`.
    pub fn new(root: OplogIndex, persistence_level: PersistenceLevel) -> Self {
        Self {
            root,
            persistence_level,
            descendants: std::collections::HashSet::new(),
            current_is_descendant_scope: false,
        }
    }
}
