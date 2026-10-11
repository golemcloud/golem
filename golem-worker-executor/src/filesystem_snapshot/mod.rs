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
//! [`FilesystemSnapshotStore`] is the interface. [`InMemorySnapshotStore`] keeps each snapshot in
//! the memory of the process. The contract suite in `contract_tests` holds the behaviour that each
//! store must have, and it compiles only for tests.

use crate::services::agent_filesystem_snapshots::StoreKey;
use async_trait::async_trait;
use futures::future::BoxFuture;
use golem_common::model::{AgentFingerprint, OwnedAgentId, Timestamp};
use golem_service_base::storage::blob::BlobStorageNamespace;
use std::cmp::Reverse;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod agent_work;
mod clock;
#[cfg(test)]
mod contract_tests;
#[cfg(any(test, feature = "test-utils"))]
mod memory;
mod rustic;
#[cfg(test)]
mod time_zone_tests;

#[cfg(any(test, feature = "test-utils"))]
pub(crate) use memory::InMemorySnapshotStore;
#[cfg(test)]
pub(crate) use memory::SpacedTimes;
use rustic::RusticSnapshotStore;
#[cfg(any(test, feature = "test-utils"))]
pub(crate) use rustic::run_delay;

/// The filesystem snapshots of one incarnation of an agent.
///
/// Each incarnation has one value: the agent with the fingerprint of the incarnation. A new
/// incarnation of the same agent id has another fingerprint, so it never shares the snapshots of
/// an earlier one. The value is opaque outside this module. Many owners hold it, so a clone shares
/// the namespace instead of copying it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AgentSnapshots(std::sync::Arc<BlobStorageNamespace>);

impl AgentSnapshots {
    /// Gives the filesystem snapshots of the incarnation `fingerprint` of the agent.
    pub(crate) fn agent(agent: &OwnedAgentId, fingerprint: AgentFingerprint) -> Self {
        Self(std::sync::Arc::new(
            BlobStorageNamespace::FilesystemSnapshots {
                environment_id: agent.environment_id,
                agent_id: agent.agent_id.clone(),
                fingerprint,
            },
        ))
    }
}

/// The name of one filesystem snapshot of an agent.
///
/// A name has 1 to 64 characters, and each character is an ASCII letter, an ASCII digit, `-` or
/// `_`. So a name can be one segment of a path, and it can be a label.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SnapshotName(Box<str>);

impl SnapshotName {
    /// The largest number of characters in a name.
    const MAX_LENGTH: usize = 64;

    /// Gives the name with the text `text`, or an error when the text breaks a rule of a name.
    pub(crate) fn new(text: &str) -> Result<Self, InvalidSnapshotName> {
        let valid = (1..=Self::MAX_LENGTH).contains(&text.len())
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if valid {
            Ok(Self(text.into()))
        } else {
            Err(InvalidSnapshotName { text: text.into() })
        }
    }

    /// Gives the text of the name.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

// The hash and the order of a name are those of its text, so a set of names can be searched by
// the text.
impl std::borrow::Borrow<str> for SnapshotName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Display for SnapshotName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A text that breaks a rule of [`SnapshotName`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvalidSnapshotName {
    text: Box<str>,
}

impl Display for InvalidSnapshotName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the filesystem snapshot name {:?} does not have 1 to {} ASCII letters, digits, `-` or `_`",
            self.text,
            SnapshotName::MAX_LENGTH
        )
    }
}

impl std::error::Error for InvalidSnapshotName {}

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

/// Why the limiter of a call withdrew the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Withdrawal {
    /// The caller stopped the call, or the shutdown came.
    Stopped,
    /// The deadline of the waits of the call passed.
    Deadline,
}

/// How a withdrawal ends a store call whose last failed run gave `F`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WithdrawnCall<F> {
    /// The call gives `Failed` with the failure of its last run.
    Failed(F),
    /// The call gives `Stopped` with the cause.
    Stopped(Withdrawal),
}

/// Gives how a withdrawal with `cause` ends a call whose last run failed on the storage with
/// `last`, or that has no failed run. The deadline gives `Failed` with `last` when there is one;
/// each other case gives `Stopped` with the cause.
pub(crate) fn withdrawn_call<F>(cause: Withdrawal, last: Option<F>) -> WithdrawnCall<F> {
    match (cause, last) {
        (Withdrawal::Deadline, Some(last)) => WithdrawnCall::Failed(last),
        (cause, _) => WithdrawnCall::Stopped(cause),
    }
}

/// The slots of one store call, from the limiter of the caller. The store takes one slot for each
/// run of the call, before the run starts, and gives it back when the run ends.
pub(crate) trait RunSlots: Send + Sync {
    /// Gives a slot, or the withdrawal of the call. `immediate` is true for the first take of the
    /// call and for a take that no wait came before.
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>>;

    /// Completes when the caller withdraws the call, with the cause.
    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal>;

    /// The call waits for its next run after a run that failed, and no write of the call can
    /// still land.
    fn waiting_after_failure(&self) {}

    /// The call only waits until its writes that can still land have landed or can no longer
    /// land. It holds no slot. It starts no run and no write until the limiter grants it a new
    /// slot.
    fn waiting_for_late_writes(&self) {}
}

/// A slot of a [`RunSlots`]. When it drops, it goes back to the limiter.
pub(crate) struct Slot(#[allow(dead_code)] Box<dyn Send + Sync>);

impl Slot {
    /// Gives the slot that `held` keeps. The slot goes back when `held` drops.
    pub(crate) fn new(held: impl Send + Sync + 'static) -> Self {
        Self(Box::new(held))
    }
}

/// The store could not finish a call. A later call can succeed. The doc of each method names the
/// causes, and says what a failed call can leave.
#[derive(Debug)]
pub(crate) struct Failed(anyhow::Error);

impl Failed {
    /// Gives the failure with its cause.
    pub(crate) fn new(cause: anyhow::Error) -> Self {
        Self(cause)
    }

    /// Gives the cause of the failure.
    #[cfg(test)]
    pub(crate) fn cause(&self) -> &anyhow::Error {
        &self.0
    }
}

impl Display for Failed {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the filesystem snapshot store could not finish the call: {:#}",
            self.0
        )
    }
}

impl std::error::Error for Failed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Why a call that only the limiter or the storage can end did not succeed.
#[derive(Debug)]
pub(crate) enum CallError {
    /// The limiter withdrew the call, or the store is shut down.
    Stopped(Withdrawal),
    Failed(Failed),
}

/// Why a save did not succeed.
#[derive(Debug)]
pub(crate) enum SaveError {
    /// Another snapshot of the agent has the name.
    NameInUse,
    /// The save could not read the tree.
    Source(std::io::Error),
    /// The limiter withdrew the call, the cancel of the save fired, or the store is shut down.
    Stopped(Withdrawal),
    Failed(Failed),
}

/// Why a restore did not succeed.
#[derive(Debug)]
pub(crate) enum RestoreFailure {
    /// No complete snapshot has the name.
    NotFound,
    /// An integrity check failed.
    Corrupt(anyhow::Error),
    /// The directory could not take the tree.
    Destination(std::io::Error),
    /// The limiter withdrew the call, or the store is shut down.
    Stopped(Withdrawal),
    Failed(Failed),
}

/// Why a read without a limiter did not succeed.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// An integrity check failed.
    Corrupt(anyhow::Error),
    /// The store is shut down.
    Stopped,
    Failed(Failed),
}

/// The failure of an error of a call, for the tests that look at it.
#[cfg(test)]
pub(crate) trait FailureOf {
    /// Gives the cause of a `Failed` error, and `None` for each other error.
    fn failure(&self) -> Option<&anyhow::Error>;

    /// Tells whether the error is `Stopped`.
    fn stopped(&self) -> bool;
}

#[cfg(test)]
impl FailureOf for CallError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::Stopped(_) => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

#[cfg(test)]
impl FailureOf for SaveError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::NameInUse | Self::Source(_) | Self::Stopped(_) => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

#[cfg(test)]
impl FailureOf for RestoreFailure {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::NotFound | Self::Corrupt(_) | Self::Destination(_) | Self::Stopped(_) => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

#[cfg(test)]
impl FailureOf for ReadError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::Corrupt(_) | Self::Stopped => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped)
    }
}

impl Display for Withdrawal {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Stopped => "the filesystem snapshot call was stopped",
            Self::Deadline => "the deadline of the filesystem snapshot call passed",
        })
    }
}

impl Display for CallError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped(cause) => write!(formatter, "{cause}"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for CallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Stopped(_) => None,
            Self::Failed(failed) => Some(failed),
        }
    }
}

impl Display for SaveError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameInUse => formatter.write_str("a filesystem snapshot already has the name"),
            Self::Source(error) => write!(
                formatter,
                "failed to read the tree of the filesystem snapshot: {error}"
            ),
            Self::Stopped(cause) => write!(formatter, "{cause}"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NameInUse | Self::Stopped(_) => None,
            Self::Source(error) => Some(error),
            Self::Failed(failed) => Some(failed),
        }
    }
}

impl Display for RestoreFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no complete filesystem snapshot has the name"),
            Self::Corrupt(source) => write!(
                formatter,
                "a filesystem snapshot failed an integrity check: {source:#}"
            ),
            Self::Destination(error) => write!(
                formatter,
                "failed to write the tree of the filesystem snapshot: {error}"
            ),
            Self::Stopped(cause) => write!(formatter, "{cause}"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for RestoreFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound | Self::Stopped(_) => None,
            Self::Corrupt(source) => Some(source.as_ref()),
            Self::Destination(error) => Some(error),
            Self::Failed(failed) => Some(failed),
        }
    }
}

impl Display for ReadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corrupt(source) => write!(
                formatter,
                "a filesystem snapshot failed an integrity check: {source:#}"
            ),
            Self::Stopped => formatter.write_str("the filesystem snapshot store is shut down"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Corrupt(source) => Some(source.as_ref()),
            Self::Stopped => None,
            Self::Failed(failed) => Some(failed),
        }
    }
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
    /// earlier run wrote, the data of the snapshot moved more often during the call than the store
    /// runs it again, or a part of the data of the snapshot is marked for deletion while a later
    /// snapshot reuses it; then a later restore can succeed. A restore of a name that a delete
    /// removes at the same time gives the whole tree or `NotFound`. `NotFound` and `Corrupt` give
    /// the same answer for every new try of the name, with one exception: a name whose save gave
    /// an error can give `NotFound` and later the whole tree, as the doc of `save` says. A failed
    /// restore can leave a part of the tree in `into`; after `Failed`, a new try into an empty
    /// directory can succeed. The restore never follows a symbolic link of the tree when it
    /// removes what an earlier run wrote, and never changes a path outside `into`.
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
    rustic::STORAGE_CALL_DEADLINES
}

/// A limiter that gives a slot to each take and never withdraws a call: the limiter of a `stat`,
/// which takes no slot, and of the calls that a test makes on a store outside the service.
pub(crate) struct Unlimited;

impl RunSlots for Unlimited {
    fn take(&self, _immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        Box::pin(std::future::ready(Ok(Slot::new(()))))
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(std::future::pending())
    }
}

/// A cancel of a save that never fires.
#[cfg(test)]
pub(crate) fn never_cancelled() -> &'static CancellationToken {
    static NEVER: std::sync::LazyLock<CancellationToken> =
        std::sync::LazyLock::new(CancellationToken::new);
    &NEVER
}

/// Gives the store of the filesystem snapshots over `storage`, with the key and the settings of
/// `config`. Only the holder of the store calls of the service can make a [`StoreKey`], so only it
/// makes the store.
pub(crate) fn managed_store(
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

#[cfg(test)]
mod tests {
    use super::{
        CallError, Failed, ReadError, RestoreFailure, SaveError, SnapshotInfo, SnapshotName,
        Withdrawal, newest_first, snapshot_time,
    };
    use golem_common::model::Timestamp;
    use pretty_assertions::assert_eq;
    use std::io::ErrorKind;
    use test_r::test;

    fn name(text: &str) -> SnapshotName {
        SnapshotName::new(text).unwrap()
    }

    fn info(created_at: u64) -> SnapshotInfo {
        SnapshotInfo {
            created_at: Timestamp::from(created_at),
            files: 0,
            bytes: 0,
        }
    }

    #[test]
    fn a_snapshot_name_has_1_to_64_ascii_letters_digits_dashes_or_underscores() {
        let accepted = [
            "p-8c1e3b0a-4c64-4e1e-9d4f-2f1c9a7b6e55".to_string(),
            "u-8c1e3b0a-4c64-4e1e-9d4f-2f1c9a7b6e55".to_string(),
            "a".to_string(),
            "Z_9".to_string(),
            "x".repeat(64),
        ];
        let refused = [
            String::new(),
            "x".repeat(65),
            "a/b".to_string(),
            ".".to_string(),
            "..".to_string(),
            "é".to_string(),
            " a".to_string(),
            "a.b".to_string(),
        ];

        assert_eq!(
            (
                accepted
                    .iter()
                    .map(|text| SnapshotName::new(text).map(|name| name.as_str().to_string()))
                    .collect::<Vec<_>>(),
                refused
                    .iter()
                    .map(|text| SnapshotName::new(text).is_err())
                    .collect::<Vec<_>>()
            ),
            (
                accepted.iter().cloned().map(Ok).collect::<Vec<_>>(),
                refused.iter().map(|_| true).collect::<Vec<_>>()
            )
        );
    }

    #[test]
    fn a_name_and_the_errors_say_what_they_hold() {
        let invalid = SnapshotName::new("a/b").unwrap_err();
        let failed = SaveError::Failed(Failed::new(anyhow::anyhow!("the bucket is gone")));
        let source = SaveError::Source(std::io::Error::new(ErrorKind::NotFound, "no tree"));

        assert_eq!(
            (
                name("p-1").to_string(),
                invalid.to_string(),
                failed.to_string(),
                std::error::Error::source(&failed).map(ToString::to_string),
                std::error::Error::source(&source).map(ToString::to_string),
                std::error::Error::source(&RestoreFailure::NotFound).is_none(),
                CallError::Stopped(Withdrawal::Deadline).to_string(),
            ),
            (
                "p-1".to_string(),
                "the filesystem snapshot name \"a/b\" does not have 1 to 64 ASCII letters, digits, `-` or `_`".to_string(),
                "the filesystem snapshot store could not finish the call: the bucket is gone".to_string(),
                Some("the filesystem snapshot store could not finish the call: the bucket is gone".to_string()),
                Some("no tree".to_string()),
                true,
                "the deadline of the filesystem snapshot call passed".to_string(),
            )
        );
    }

    /// The old error of every method, `SnapshotStoreError`, had 16 bytes. Each error of the
    /// methods stays at that size.
    #[test]
    fn each_error_of_the_store_has_at_most_16_bytes() {
        assert_eq!(
            [
                std::mem::size_of::<Failed>(),
                std::mem::size_of::<CallError>(),
                std::mem::size_of::<SaveError>(),
                std::mem::size_of::<RestoreFailure>(),
                std::mem::size_of::<ReadError>(),
                std::mem::size_of::<Result<SnapshotInfo, SaveError>>(),
            ],
            [8, 16, 16, 16, 16, 32]
        );
    }

    #[test]
    fn newest_first_orders_by_time_and_then_by_name_both_in_reverse() {
        let listed = newest_first([
            (name("b"), info(10)),
            (name("a"), info(20)),
            (name("c"), info(10)),
            (name("d"), info(5)),
        ]);

        assert_eq!(
            listed.into_vec(),
            vec![
                (name("a"), info(20)),
                (name("c"), info(10)),
                (name("b"), info(10)),
                (name("d"), info(5)),
            ]
        );
    }

    #[test]
    fn a_snapshot_time_is_later_than_the_newest_snapshot_of_the_scope() {
        let now = Timestamp::from(1_000);

        assert_eq!(
            (
                snapshot_time(now, None),
                snapshot_time(now, Some(Timestamp::from(999))),
                snapshot_time(now, Some(Timestamp::from(1_000))),
                snapshot_time(now, Some(Timestamp::from(5_000))),
            ),
            (now, now, Timestamp::from(1_001), Timestamp::from(5_001))
        );
    }

    #[test]
    fn a_withdrawal_stops_a_call_and_its_deadline_fails_a_call_whose_run_failed() {
        assert_eq!(
            [
                super::withdrawn_call(super::Withdrawal::Stopped, None),
                super::withdrawn_call(super::Withdrawal::Stopped, Some("last")),
                super::withdrawn_call(super::Withdrawal::Deadline, None),
                super::withdrawn_call(super::Withdrawal::Deadline, Some("last")),
            ],
            [
                super::WithdrawnCall::Stopped(super::Withdrawal::Stopped),
                super::WithdrawnCall::Stopped(super::Withdrawal::Stopped),
                super::WithdrawnCall::Stopped(super::Withdrawal::Deadline),
                super::WithdrawnCall::Failed("last"),
            ]
        );
    }
}
