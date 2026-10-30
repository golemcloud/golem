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

//! Saves a directory tree as a named filesystem snapshot, and restores it.
//!
//! [`FilesystemSnapshotStore`] is the interface. The shared contract cases in `tests.rs`
//! exercise both the rustic and in-memory adapters through that interface.

use crate::services::agent_filesystem_snapshots::StoreKey;
use async_trait::async_trait;
use golem_common::model::Timestamp;
use std::cmp::Reverse;
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod agent_snapshots;
mod backend;
mod clock;
mod errors;
mod operation_drain;
mod run_admission;
mod snapshot_name;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

pub(crate) use agent_snapshots::AgentSnapshots;
#[cfg(any(test, feature = "test-utils"))]
pub(crate) use backend::in_memory::InMemorySnapshotStore;
use backend::rustic::RusticSnapshotStore;
#[cfg(any(test, feature = "test-utils"))]
pub(crate) use backend::rustic::run_delay;
pub(crate) use errors::{CallError, Failed, ReadError, RestoreFailure, SaveError};
pub(crate) use run_admission::{
    RunSlots, Slot, Unlimited, Withdrawal, WithdrawnCall, withdrawn_call,
};
pub(crate) use snapshot_name::{InvalidSnapshotName, SnapshotName};

/// What a snapshot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SnapshotInfo {
    /// The time of the save.
    pub created_at: Timestamp,
    /// The number of names of regular files in the tree.
    pub files: u64,
    /// The sum of the sizes of the regular files in the tree, in bytes. Each name counts.
    pub bytes: u64,
}

/// How a save with a parent finds the files that did not change since the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeDetection {
    /// Compares each file with the parent by size and modification time.
    SizeMtime,
    /// Reads every file. A save without a parent reads every file too, so production gives no
    /// parent instead of this variant.
    #[allow(dead_code)]
    Full,
}

/// Keeps directory trees as named filesystem snapshots, separately for each agent.
///
/// A snapshot keeps the relative path and the kind of each entry below the tree. It also keeps the
/// content of a file, the target of a symlink, the permission bits and the modification time. It
/// does not keep the owner, the access time, extended attributes, or the metadata of the root of
/// the tree. An entry that is not a regular file, a directory or a symlink is outside this
/// contract.
///
/// The snapshots of one agent can have more than one writer at the same time, on one executor or
/// on several. A call does not wait for another call, except `delete_all`, which waits for the
/// calls of this store for the agent that began before it.
///
/// Each call gives a final answer. The store tries a failed storage call again for a short time,
/// runs the call again after a wait when the failure stays, and does again the work that another
/// call of the agent made invalid. When a write of a save or of a copy ends without an answer, the
/// store waits until that write has landed or can no longer land before it runs the call again or
/// answers, unless the store shuts down. `Failed` means that the store could not
/// finish the call and that a later call can succeed. The doc of each method names the causes of
/// `Failed`, and says what a failed call can leave.
///
/// Each call except `stat` gets the limiter of its call. Before a call waits for its next run
/// after a run that failed, it tells its limiter. Each run of the call takes a slot of that limiter
/// before it starts, and gives the slot back when it ends. When the limiter withdraws the call
/// before a run, or while the call waits between two runs, the call ends: it gives `Stopped` with
/// the cause, or `Failed` when the cause is the deadline and a run of the call failed before. A
/// withdrawal does not end a wait for a write that can still land; a shutdown does. A store that
/// is shut down gives `Stopped`.
///
/// No method blocks the async runtime. `save` costs the bytes that changed since the last
/// snapshot of the agent, plus one metadata read for each file. `restore` costs the size of the
/// tree. `stat` and `list` cost one read of the metadata of each snapshot of the agent.
#[async_trait]
pub(crate) trait FilesystemSnapshotStore: Send + Sync {
    /// Saves a directory tree as a snapshot with the name, and waits until it is durable.
    ///
    /// A snapshot appears in one step. After the call gives the info, the name resolves to the
    /// whole tree, on every executor. The causes of `Failed` are: the storage failed in each run,
    /// the storage gave a failure that no try can fix, or the save would have taken longer than
    /// the store allows. A save that gives an error publishes nothing, with two exceptions: when
    /// the store could not read the snapshots of the agent after a publish of the call began, or
    /// when the store shut down after a publish of the call began. Then the name can resolve to
    /// the whole tree later, and never to a part of it: `stat` and `list` show it, and a new save
    /// of the name gives `NameInUse`. A save that finds another snapshot with its name, which
    /// another writer made, gives `NameInUse`. A save can leave data that no snapshot uses. When
    /// the call gives the info, no write of the call can still land, unless the store shut down
    /// while the call waited for such a write.
    ///
    /// The tree must not change while the call runs. The store reads it while the call runs.
    /// Symlinks are read and not followed. A tree that the store cannot read gives `Source`, and
    /// so does a `tree` that is not a directory.
    ///
    /// The result gives the number of files, the size of the tree, and the time of the snapshot.
    /// That time is later than the time of each snapshot that the agent had when the save started.
    ///
    /// `parent` names the snapshot that the save can compare with. With `SizeMtime`, the store can
    /// keep the content of the parent for a file whose size and modification time equal those of
    /// the same path in the parent, and then it does not read that file. So such a file that
    /// changed can keep the content of the parent. A store can also read each file. With `Full`,
    /// the save reads every file. A parent that the agent does not have gives a save that reads
    /// every file.
    ///
    /// `cancel` stops the save: a save whose `cancel` fires before its publish begins publishes
    /// nothing and gives `Stopped`. The call returns only after the store stopped reading `tree`.
    async fn save(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, SaveError>;

    /// Rebuilds a saved tree in the empty directory `into`.
    ///
    /// The result is the tree as it was at the save: the same files, directories, symlinks,
    /// permissions and modification times. Each name of a file comes back as a separate file. A
    /// snapshot does not keep the metadata of the root of the tree. So the call does not set the
    /// permissions or the modification time of `into`. An `into` that is missing, that is not a
    /// directory, or that is not empty gives `Destination`, and the call writes nothing.
    /// `Destination` also means that `into` could not take the tree, for example because the
    /// volume is full.
    ///
    /// A restore of a name that no delete removes gives the whole tree also while saves and
    /// deletes of the agent run. The causes of `Failed` for such a name are: the storage failed in
    /// each run, the storage gave a failure that no try can fix, the storage listed a part of the
    /// data that it does not hold in each run, the store could not remove from `into` what an
    /// earlier run wrote, or the data of the snapshot moved more often during the call than the
    /// store runs it again. A restore of a name that a delete removes at the same time gives the
    /// whole tree or `NotFound`. `NotFound` and `Corrupt` give the same answer for every new try of
    /// the name, with one exception: a name whose save gave an error can give `NotFound` and later
    /// the whole tree, as the doc of `save` says. A failed restore can leave a part of the tree in
    /// `into`; after `Failed`, a new try into an empty directory can succeed. The restore never
    /// follows a symbolic link of the tree when it removes what an earlier run wrote, and never
    /// changes a path outside `into`.
    async fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, RestoreFailure>;

    /// Tells whether a name resolves to a complete snapshot, without a read of the tree.
    ///
    /// `Some` means that the snapshot is complete, and the info describes the saved tree. A
    /// restore of it gives the whole tree or one of the `Failed` causes that the doc of `restore`
    /// names. `None` means that no save of the name finished, or that the snapshot was
    /// deleted. A snapshot whose metadata fails an integrity check gives `Corrupt`. A name that no
    /// snapshot has also gives `Corrupt` when the metadata of another snapshot of the agent fails
    /// its integrity check, because that snapshot can have the name. A snapshot that a save or a
    /// delete changes during the call can be found or not found. The call takes no slot. A store
    /// that is shut down gives `Stopped`.
    async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, ReadError>;

    /// Gives every complete snapshot of the agent with its info, newest first.
    ///
    /// The order follows `created_at`. Snapshots with the same `created_at` come in the reverse
    /// order of their names. Unfinished saves, deleted snapshots, and snapshots whose metadata
    /// fails an integrity check are absent. A snapshot that exists for the whole call is in the
    /// result, and a snapshot that a save or a delete adds or removes during the call can be
    /// present or absent, so the result need not be the set of one moment.
    async fn list(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError>;

    /// Deletes the snapshots with the names `names`, as one batch.
    ///
    /// Each name stops resolving when the call gives success, so no later restore of it can
    /// succeed, unless a save of that name that runs at the same time, that gave an error, or that
    /// gave the info after the store shut down, publishes it later, as the doc of `save` says. The
    /// call is idempotent for each name. Every other snapshot of the agent continues to work, also
    /// when it shares data with a deleted one, and a save or a restore that runs at the same time
    /// gives one of the answers that its own doc names. The causes of `Failed` are: the storage
    /// failed in each run, or the storage gave a failure that no try can fix. A delete that gives
    /// an error can have deleted a part of the batch; a new call deletes the rest. Each run reads
    /// the snapshots of the agent once for the whole batch.
    async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: &[SnapshotName],
        slots: &dyn RunSlots,
    ) -> Result<(), CallError>;

    /// Removes every snapshot of the agent, with all their data and metadata.
    ///
    /// When the call gives success, each call of this store for the agent that began before it
    /// has ended, and none of them changes the snapshots of the agent afterwards. A delete of
    /// the call whose try got no answer can still land up to one storage call deadline after the
    /// call gave success, and a delete of a directory has no such bound. So a caller never saves
    /// into, copies into, or otherwise writes the agent after a delete of all its snapshots; each
    /// incarnation of an agent and each fork stage has its own fingerprint, so a caller never
    /// needs to. A call that gives an error can leave some of the snapshots; a new call removes
    /// them.
    async fn delete_all(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError>;

    /// Copies every snapshot of the agent `from` to the agent `to`, which has none.
    ///
    /// After success, `to` has the snapshots that `from` had when the last run of the call listed
    /// them, and each of them restores as the snapshot of `from` does. A snapshot that a delete of
    /// `from` removed after that listing can stay in `to`, whole. When the snapshots of `from`
    /// are gone, `to` has none. A copy that gives an error leaves `to` without snapshots, unless
    /// the config write of its last run ended without an answer and landed afterwards. The
    /// snapshots of the two agents are independent afterwards, so a save or a delete for one has
    /// no effect on the other.
    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError>;

    /// Stops the work of the store and waits until it ends. Each later operation gives `Stopped`.
    /// A store that runs no work of its own does nothing.
    async fn shut_down(&self) {}
}

/// The range of the storage call deadline that the store accepts: at least one second, so a
/// publish has time for one try, and at most one eighth of the grace period of the store, so a
/// save has time for its backup.
pub(crate) fn storage_call_deadlines() -> std::ops::RangeInclusive<Duration> {
    backend::rustic::STORAGE_CALL_DEADLINES
}

/// Gives the store of the filesystem snapshots over `storage`, with the key and the settings of
/// `config`. Only the holder of the store calls of the service can make a [`StoreKey`], so only it
/// makes the store.
pub(crate) fn create_store(
    storage: std::sync::Arc<dyn golem_service_base::storage::blob::BlobStorage>,
    config: &crate::services::golem_config::FilesystemSnapshotStoreConfig,
    _key: StoreKey,
) -> std::sync::Arc<dyn FilesystemSnapshotStore> {
    std::sync::Arc::new(RusticSnapshotStore::new(storage, config))
}

/// Gives the snapshots newest first: in the reverse order of `created_at`, and in the reverse
/// order of their names where `created_at` is the same.
fn newest_first(
    snapshots: impl IntoIterator<Item = (SnapshotName, SnapshotInfo)>,
) -> Box<[(SnapshotName, SnapshotInfo)]> {
    let mut snapshots = snapshots.into_iter().collect::<Vec<_>>();
    snapshots.sort_by(|(left_name, left), (right_name, right)| {
        (Reverse(left.created_at), Reverse(left_name))
            .cmp(&(Reverse(right.created_at), Reverse(right_name)))
    });
    snapshots.into_boxed_slice()
}

/// Gives the time of a new snapshot: `now`, or one millisecond after `newest` when `now` is not
/// later than `newest`.
///
/// `newest` is the time of the newest snapshot of the agent when the save starts. So the time of
/// a new snapshot is later than the time of each snapshot of the agent. This holds also
/// when the clocks of two executors differ.
fn snapshot_time(now: Timestamp, newest: Option<Timestamp>) -> Timestamp {
    newest.map_or(now, |newest| {
        now.max(Timestamp::from(newest.to_millis().saturating_add(1)))
    })
}
