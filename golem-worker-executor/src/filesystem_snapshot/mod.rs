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

use async_trait::async_trait;
use golem_common::model::{OwnedAgentId, Timestamp};
use golem_service_base::storage::blob::BlobStorageNamespace;
use std::cmp::Reverse;
use std::fmt::{Display, Formatter};
use std::path::Path;

#[cfg(all(target_os = "linux", any(test, feature = "fs-snapshot-benchmark")))]
pub(crate) mod benchmark;
#[cfg(test)]
mod contract_tests;
mod memory;
mod rustic;
#[cfg(test)]
mod time_zone_tests;

#[allow(unused_imports)]
pub(crate) use memory::InMemorySnapshotStore;
#[allow(unused_imports)]
pub(crate) use rustic::RusticSnapshotStore;

/// The place of the filesystem snapshots of one agent.
///
/// Each agent has one scope. A scope is opaque outside this module.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SnapshotScope(BlobStorageNamespace);

impl SnapshotScope {
    /// Gives the scope of the agent.
    pub(crate) fn agent(agent: &OwnedAgentId) -> Self {
        Self(BlobStorageNamespace::FilesystemSnapshots {
            environment_id: agent.environment_id,
            agent_id: agent.agent_id.clone(),
        })
    }
}

/// The name of one filesystem snapshot in a scope.
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

/// Why a call on a [`FilesystemSnapshotStore`] failed.
#[derive(Debug)]
pub(crate) enum SnapshotStoreError {
    /// No complete snapshot has the name.
    NotFound,
    /// A snapshot already has the name.
    AlreadyExists,
    /// The save could not read the tree.
    Source(std::io::Error),
    /// The restore could not write the tree into the directory.
    Destination(std::io::Error),
    /// The storage of the snapshots failed. `retryable` tells whether a new attempt can succeed
    /// without a change.
    Storage {
        retryable: bool,
        source: anyhow::Error,
    },
    /// An integrity check failed.
    Corrupt(anyhow::Error),
}

impl Display for SnapshotStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no complete filesystem snapshot has the name"),
            Self::AlreadyExists => {
                formatter.write_str("a filesystem snapshot already has the name")
            }
            Self::Source(error) => write!(
                formatter,
                "failed to read the tree of the filesystem snapshot: {error}"
            ),
            Self::Destination(error) => write!(
                formatter,
                "failed to write the tree of the filesystem snapshot: {error}"
            ),
            Self::Storage { source, .. } => write!(
                formatter,
                "the storage of the filesystem snapshots failed: {source:#}"
            ),
            Self::Corrupt(source) => write!(
                formatter,
                "a filesystem snapshot failed an integrity check: {source:#}"
            ),
        }
    }
}

impl std::error::Error for SnapshotStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound | Self::AlreadyExists => None,
            Self::Source(error) | Self::Destination(error) => Some(error),
            Self::Storage { source, .. } | Self::Corrupt(source) => Some(source.as_ref()),
        }
    }
}

/// Keeps directory trees as named filesystem snapshots, one scope for each agent.
///
/// A snapshot keeps the relative path and the kind of each entry below the tree. It also keeps the
/// content of a file, the target of a symlink, the permission bits and the modification time. It
/// does not keep the owner, the access time, extended attributes, or the metadata of the root of
/// the tree. An entry that is not a regular file, a directory or a symlink is outside this
/// contract.
///
/// One scope can have more than one writer at the same time, and no method locks. A delete of one
/// name never damages a restore of another name. A restore of a name that is deleted at the same
/// time gives the whole tree, `NotFound` or `Corrupt`. No method blocks the async runtime. `save`
/// costs the bytes that changed since the last snapshot in the scope, plus one metadata read for
/// each file. `restore` costs the size of the tree. `stat` and `list` cost a few small reads.
#[async_trait]
pub(crate) trait FilesystemSnapshotStore: Send + Sync {
    /// Saves a directory tree as a snapshot with the name, and waits until it is durable.
    ///
    /// A snapshot appears in one step. Before the call returns, the name resolves to nothing. After
    /// it returns, the name resolves to the whole tree, on every executor. An interrupted save
    /// publishes nothing and leaves the name free for a new attempt. A save is interrupted when it
    /// fails, when its process stops, or when the caller drops the call before it returns. It can
    /// leave data that no snapshot uses, which a later save or delete in the scope removes.
    ///
    /// The tree must not change while the call runs. The store reads it and locks nothing.
    /// Symlinks are read and not followed. A name that is already in use gives `AlreadyExists`
    /// and changes nothing. A tree that the store cannot read gives `Source`, and so does a
    /// `tree` that is not a directory.
    ///
    /// The result gives the number of files, the size of the tree, and the time of the
    /// snapshot. That time is later than the time of each snapshot that the scope held when the
    /// save started.
    async fn save(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        tree: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError>;

    /// Rebuilds a saved tree in the empty directory `into`.
    ///
    /// The result is the tree as it was at the save: the same files, directories, symlinks,
    /// permissions and modification times. Each name of a file comes back as a separate file. A
    /// snapshot does not keep the metadata of the root of the tree. So the call does not set the
    /// permissions or the modification time of `into`. A failed restore can leave a part of the
    /// tree in `into`, so only a successful restore gives a whole tree.
    ///
    /// `NotFound` means that no complete snapshot has the name, because no save of it finished
    /// or the snapshot was deleted. `Corrupt` means that an integrity check failed. Both give
    /// the same answer for every new try of that name. `Destination` means that `into` could not
    /// take the tree, for example because the volume is full. An `into` that is missing, that is
    /// not a directory, or that is not empty gives `Destination`, and the call writes nothing.
    async fn restore(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError>;

    /// Tells whether a name resolves to a complete snapshot, without a read of the tree.
    ///
    /// `Some` means that a restore of the name finds all that it needs, and the info describes
    /// the saved tree. `None` means that no save of the name finished, or that the snapshot was
    /// deleted. A snapshot whose metadata fails an integrity check gives `Corrupt`.
    async fn stat(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError>;

    /// Gives every complete snapshot in the scope with its info, newest first.
    ///
    /// The order follows `created_at`. Snapshots with the same `created_at` come in the reverse
    /// order of their names. Unfinished saves, deleted snapshots, and snapshots whose metadata
    /// fails an integrity check are absent. The scope can change immediately after the call, so
    /// the result shows one moment.
    async fn list(
        &self,
        scope: &SnapshotScope,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError>;

    /// Deletes one snapshot.
    ///
    /// The name stops resolving immediately, so no later restore of it can succeed. The call is
    /// idempotent: an unknown name, or a name that is already deleted, gives success. Every other
    /// snapshot in the scope continues to work, also when it shares data with the deleted one.
    /// Storage comes back after a grace period, and a restore that is already in progress is not
    /// disturbed.
    async fn delete(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError>;

    /// Removes a scope with all that is in it, including its own metadata.
    ///
    /// After the call, the scope is as unused as it was before its first save, and a later save
    /// creates it again. The call is idempotent and needs no list of names. A save into the scope
    /// at the same time is not cancelled, and can make the scope live again.
    async fn delete_scope(&self, scope: &SnapshotScope) -> Result<(), SnapshotStoreError>;

    /// Copies every snapshot of the scope `from` into the empty scope `to`.
    ///
    /// The scope `to` then has the same names, and each name gives the same tree and the same
    /// info. The scope `from` does not change. The two scopes are independent afterwards, so a
    /// save or a delete in one has no effect on the other. Only the data that `to` does not have
    /// moves. A copy into a scope that is not empty is outside this contract.
    async fn copy_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError>;
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
/// `newest` is the time of the newest snapshot that the scope holds when the save starts. So the
/// time of a new snapshot is later than the time of each snapshot in the scope. This holds also
/// when the clocks of two executors differ.
fn snapshot_time(now: Timestamp, newest: Option<Timestamp>) -> Timestamp {
    newest.map_or(now, |newest| {
        now.max(Timestamp::from(newest.to_millis().saturating_add(1)))
    })
}

#[cfg(test)]
mod tests {
    use super::{SnapshotInfo, SnapshotName, SnapshotStoreError, newest_first, snapshot_time};
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
        let storage = SnapshotStoreError::Storage {
            retryable: true,
            source: anyhow::anyhow!("the bucket is gone"),
        };
        let source =
            SnapshotStoreError::Source(std::io::Error::new(ErrorKind::NotFound, "no tree"));

        assert_eq!(
            (
                name("p-1").to_string(),
                invalid.to_string(),
                storage.to_string(),
                std::error::Error::source(&storage).map(ToString::to_string),
                std::error::Error::source(&source).map(ToString::to_string),
                std::error::Error::source(&SnapshotStoreError::NotFound).is_none(),
            ),
            (
                "p-1".to_string(),
                "the filesystem snapshot name \"a/b\" does not have 1 to 64 ASCII letters, digits, `-` or `_`".to_string(),
                "the storage of the filesystem snapshots failed: the bucket is gone".to_string(),
                Some("the bucket is gone".to_string()),
                Some("no tree".to_string()),
                true,
            )
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
}
