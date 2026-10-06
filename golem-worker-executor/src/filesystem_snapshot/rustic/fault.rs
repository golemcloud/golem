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

//! The errors that the backend puts into the chain of a rustic error, and the classification of a
//! failed try of a blob call and of a failed run.
//!
//! rustic gives no public kind of an error. So the classification reads the chain of sources: the
//! markers of this module, the name, range and missing-blob errors of the blob storage, and the
//! I/O errors.

use super::prune::SNAPSHOTS_PATH;
use super::runs::RunEnd;
use golem_service_base::storage::blob::{BlobMissingError, BlobNameError, BlobRangeError};
use rustic_core::FileType;
use rustic_core::Id;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;

/// A blob storage call of the backend that failed, got no answer within its deadline, or did not
/// start because its operation was cancelled. The source is the failure.
#[derive(Debug)]
pub(super) struct BlobCallFailed {
    failure: anyhow::Error,
}

impl BlobCallFailed {
    pub(super) fn new(failure: anyhow::Error) -> Self {
        Self { failure }
    }
}

impl Display for BlobCallFailed {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the blob storage call gave an error")
    }
}

impl Error for BlobCallFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.failure.as_ref())
    }
}

/// The operation of the backend was cancelled, so the backend made no more calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OperationCancelled;

impl Display for OperationCancelled {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the filesystem snapshot operation was cancelled")
    }
}

impl Error for OperationCancelled {}

/// The lease of the prune ran out, so the backend made no more calls. Another delete can then take
/// the claim of the prune, so the prune must not change the repository any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LeaseExpired;

impl Display for LeaseExpired {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the lease of the prune claim ran out")
    }
}

impl Error for LeaseExpired {}

/// Tells whether an error in the chain is [`LeaseExpired`]. Only tests ask this: a prune whose
/// lease ran out fails as any other failed prune does, and the delete keeps its answer.
#[cfg(test)]
pub(super) fn is_lease_expired(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<LeaseExpired>())
}

/// Another writer made the config file of the repository first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConfigExists;

impl Display for ConfigExists {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("another writer made the config file of the repository first")
    }
}

impl Error for ConfigExists {}

/// The blob storage holds no file at the path that rustic reads, for example because a delete
/// removed it after a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileMissing {
    /// The path of the file, relative to the root of the repository.
    pub(super) path: Box<Path>,
}

impl Display for FileMissing {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the blob storage holds no file at {}",
            self.path.display()
        )
    }
}

impl Error for FileMissing {}

/// Tells whether an error in the chain is [`FileMissing`].
pub(super) fn is_file_missing(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<FileMissing>())
}

/// Tells whether an error in the chain is [`FileMissing`] for a snapshot file.
pub(super) fn is_snapshot_missing(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| {
        error
            .downcast_ref::<FileMissing>()
            .is_some_and(|missing| missing.path.starts_with(SNAPSHOTS_PATH))
    })
}

/// Tells whether an error in the chain is [`FileMissing`] for an index file. A prune writes its new
/// index files and then deletes the old ones at once, so an operation that listed an index file
/// before a prune can find it gone at its read. A later try lists the new index files.
pub(super) fn is_index_missing(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| {
        error
            .downcast_ref::<FileMissing>()
            .is_some_and(|missing| missing.path.starts_with(FileType::Index.dirname()))
    })
}

/// Tells whether an error in the chain is [`ConfigExists`].
pub(super) fn is_config_exists(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<ConfigExists>())
}

/// Tells whether an error in the chain is a failed blob storage call.
pub(super) fn is_storage_failure(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<BlobCallFailed>())
}

/// How one try of a blob call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallFailure {
    /// The operation of the call was cancelled.
    Cancelled,
    /// The lease of the prune ran out.
    LeaseExpired,
    /// The storage gave an answer that no try can change: a name or a range that breaks a rule, a
    /// permission that the storage refuses, or a blob that is not there.
    Permanent,
    /// The try got no answer within the time left of the window of the call.
    TimedOut,
    /// Any other failure.
    Failed,
}

/// Classifies the error of one try of a blob call.
pub(super) fn call_failure(error: &anyhow::Error) -> CallFailure {
    let in_chain = |is: fn(&(dyn Error + 'static)) -> bool| chain(error.as_ref()).any(is);
    if in_chain(|error| error.is::<OperationCancelled>()) {
        CallFailure::Cancelled
    } else if in_chain(|error| error.is::<LeaseExpired>()) {
        CallFailure::LeaseExpired
    } else if in_chain(is_permanent) {
        CallFailure::Permanent
    } else if in_chain(|error| error.is::<tokio::time::error::Elapsed>()) {
        CallFailure::TimedOut
    } else {
        CallFailure::Failed
    }
}

/// Tells whether an error is an answer of the storage that no try can change.
fn is_permanent(error: &(dyn Error + 'static)) -> bool {
    error.is::<BlobNameError>()
        || error.is::<BlobRangeError>()
        || error.is::<BlobMissingError>()
        || error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
}

/// Tells whether a write whose try failed with `failure` can still land: every failure but one that
/// no try can change.
pub(super) fn may_land(failure: CallFailure) -> bool {
    failure != CallFailure::Permanent
}

/// Gives how a run ends after a blob call whose last try failed with `failure`.
pub(super) fn run_end(failure: CallFailure) -> RunEnd {
    match failure {
        CallFailure::Cancelled => RunEnd::Cancelled,
        CallFailure::Permanent => RunEnd::Permanent,
        CallFailure::LeaseExpired | CallFailure::TimedOut | CallFailure::Failed => {
            RunEnd::CallFailed
        }
    }
}

/// Gives how a run ends after a blob call that the store made without rustic failed with `error`.
pub(super) fn storage_end(error: &anyhow::Error) -> RunEnd {
    run_end(call_failure(error))
}

/// Gives how a run ends after a step of rustic failed with `error`, when the error came from a
/// blob call, or `None` when it did not.
pub(super) fn rustic_storage_end(error: &anyhow::Error) -> Option<RunEnd> {
    is_storage_failure(error.as_ref()).then(|| {
        if chain(error.as_ref()).any(|error| error.is::<OperationCancelled>()) {
            RunEnd::Cancelled
        } else if chain(error.as_ref()).any(is_permanent) {
            RunEnd::Permanent
        } else {
            RunEnd::CallFailed
        }
    })
}

/// Tells whether a range error of the blob storage is in the chain.
fn is_range_error(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<BlobRangeError>())
}

/// Gives the path of the first [`FileMissing`] in the chain.
pub(super) fn missing_file_path<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a Path> {
    chain(error).find_map(|error| {
        error
            .downcast_ref::<FileMissing>()
            .map(|missing| &*missing.path)
    })
}

/// Tells whether an error in the chain is [`FileMissing`] for a pack.
fn is_pack_missing(error: &(dyn Error + 'static)) -> bool {
    missing_file_path(error).is_some_and(|path| path.starts_with(FileType::Pack.dirname()))
}

/// Gives the start of the hex id of the blob that a lookup in the index did not find, from a "not
/// found in index" error of rustic. rustic keeps the id as the context of the error, and gives the
/// context only in the text of the error, as the text between the backticks before `not found in
/// index`. rustic writes an id there with its short display, the first 8 hex digits, so the text
/// holds the start of the id and not the whole id.
pub(super) fn missing_blob(error: &(dyn Error + 'static)) -> Option<Box<str>> {
    chain(error).find_map(|error| {
        let text = error.to_string();
        let before = &text[..text.find("` not found in index")?];
        let start = &before[before.rfind('`')? + 1..];
        (!start.is_empty()
            && start.len() <= 64
            && start.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| Box::from(start.to_ascii_lowercase()))
    })
}

/// The text with which rustic begins an error of the kind `InputOutput`.
const RUSTIC_LOCAL_IO: &str =
    "`rustic_core` experienced an error related to `input/output operations`.";

/// Gives the kind of the first I/O error in the chain. rustic gives some local I/O errors without
/// their `io::Error` as a source: the error of the kind `InputOutput` keeps the OS error only in
/// the text of its cause, as `Os { code: N, .. }`. Such an error gives the kind of the code, or
/// `Other` when its text has no code. An error of a blob call is not a local I/O error, so the
/// caller reads the storage first.
fn io_error_kind(error: &(dyn Error + 'static)) -> Option<std::io::ErrorKind> {
    chain(error)
        .find_map(|error| error.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind)
        .or_else(|| {
            chain(error)
                .map(|error| error.to_string())
                .find(|text| text.starts_with(RUSTIC_LOCAL_IO))
                .map(|text| {
                    text.split_once("Os { code: ")
                        .and_then(|(_, after)| {
                            after
                                .split(|character: char| !character.is_ascii_digit())
                                .next()
                                .and_then(|code| code.parse::<i32>().ok())
                        })
                        .map_or(std::io::ErrorKind::Other, |code| {
                            std::io::Error::from_raw_os_error(code).kind()
                        })
                })
        })
}

/// The phase of a run of a restore in which a step failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// A load of the repository, a read of the snapshots or of the index.
    Load,
    /// The check of the index for the whole tree, which writes nothing.
    Check,
    /// The write of the tree into the directory.
    Write,
}

/// What the error of a failed step of a restore is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RestoreError {
    /// A pack, with its id when the path names one, was gone at its read.
    PackMissing(Option<Id>),
    /// A file that is not a pack or the config was gone at its read: an index file, a key or a
    /// snapshot file that a prune or a delete of all snapshots removed after the listing.
    FileMissing(Box<Path>),
    /// The config of the repository was gone at its read.
    ConfigMissing,
    Cancelled,
    /// A storage call gave a failure that no try can fix; `range` for a range error.
    Permanent {
        range: bool,
    },
    /// A storage call failed after its tries.
    CallFailed,
    /// The local filesystem of the directory gave this kind of I/O error.
    Destination(std::io::ErrorKind),
    /// The index of the check does not hold a blob of the tree.
    CheckFailed,
    /// The data of the repository failed an integrity check.
    Corrupt,
}

/// Classifies the error of a failed step of a restore in `phase`. The storage decides first: a
/// missing file, then a failed storage call. A local I/O error in the check or the write is
/// `Destination`. Any other error is `CheckFailed` in the check, where the index misses a blob of
/// the tree, and `Corrupt` elsewhere: it comes from data that fails its integrity check, because
/// the store reads only data of the repository and writes only into the directory.
pub(super) fn restore_error(error: &anyhow::Error, phase: Phase) -> RestoreError {
    let config = Path::new(super::backend::CONFIG_PATH);
    if is_pack_missing(error.as_ref()) {
        return RestoreError::PackMissing(pack_of(error));
    }
    match missing_file_path(error.as_ref()) {
        Some(path) if path == config => return RestoreError::ConfigMissing,
        Some(path) => return RestoreError::FileMissing(path.into()),
        None => {}
    }
    match rustic_storage_end(error) {
        Some(RunEnd::Cancelled) => return RestoreError::Cancelled,
        Some(RunEnd::Permanent) => {
            return RestoreError::Permanent {
                range: is_range_error(error.as_ref()),
            };
        }
        Some(_) => return RestoreError::CallFailed,
        None => {}
    }
    match (phase, io_error_kind(error.as_ref())) {
        (Phase::Check | Phase::Write, Some(kind)) => RestoreError::Destination(kind),
        (Phase::Check, None) => RestoreError::CheckFailed,
        (Phase::Load, _) | (Phase::Write, None) => RestoreError::Corrupt,
    }
}

/// Gives the id of the pack that a [`super::fault::FileMissing`] of a pack names.
fn pack_of(error: &anyhow::Error) -> Option<Id> {
    missing_file_path(error.as_ref())?
        .file_name()?
        .to_str()
        .and_then(|name| name.parse::<Id>().ok())
}

/// What the error of a failed backup of a save is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaveFault {
    /// The run ends this way.
    Ended(RunEnd),
    /// The local filesystem of the tree gave this kind of I/O error.
    Source(std::io::ErrorKind),
}

/// Classifies the error of a failed backup of a save. The storage decides first: a failed storage
/// call, then an index file that was gone at its read, which a new run can find again. A local I/O
/// error is `Source`. Any other error is permanent.
pub(super) fn save_fault(error: &anyhow::Error) -> SaveFault {
    if let Some(end) = rustic_storage_end(error) {
        return SaveFault::Ended(end);
    }
    if is_index_missing(error.as_ref()) {
        return SaveFault::Ended(RunEnd::CallFailed);
    }
    match io_error_kind(error.as_ref()) {
        Some(kind) => SaveFault::Source(kind),
        None => SaveFault::Ended(RunEnd::Permanent),
    }
}

/// Gives the error and each error in its chain of sources.
fn chain<'a>(error: &'a (dyn Error + 'static)) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    std::iter::successors(Some(error), |&error| error.source())
}

#[cfg(test)]
mod tests {
    use super::{
        BlobCallFailed, CallFailure, ConfigExists, FileMissing, LeaseExpired, OperationCancelled,
        call_failure, is_config_exists, is_index_missing, is_pack_missing, may_land, missing_blob,
        missing_file_path, run_end, rustic_storage_end, storage_end,
    };
    use crate::filesystem_snapshot::rustic::runs::RunEnd;
    use golem_service_base::storage::blob::{BlobMissingError, BlobNameError, BlobRangeError};
    use pretty_assertions::assert_eq;
    use rustic_core::{ErrorKind, Id, RusticError};
    use std::io;
    use std::path::Path;
    use test_r::test;

    /// Gives a rustic error whose source is the error, as rustic gives it to the store.
    fn rustic(source: impl std::error::Error + Send + Sync + 'static) -> anyhow::Error {
        anyhow::Error::new(RusticError::with_source(
            ErrorKind::Backend,
            "the operation failed",
            source,
        ))
    }

    fn failed_call(failure: anyhow::Error) -> anyhow::Error {
        rustic(BlobCallFailed::new(failure))
    }

    fn missing(path: &str) -> anyhow::Error {
        rustic(FileMissing {
            path: Path::new(path).into(),
        })
    }

    async fn elapsed() -> anyhow::Error {
        anyhow::Error::new(
            tokio::time::timeout(std::time::Duration::ZERO, std::future::pending::<()>())
                .await
                .unwrap_err(),
        )
        .context("the blob storage gave no answer")
    }

    #[test]
    async fn each_failure_of_a_try_gets_its_class() {
        let name = || {
            anyhow::Error::new(BlobNameError::NoName {
                path: std::path::PathBuf::new(),
            })
        };
        assert_eq!(
            [
                call_failure(&anyhow::Error::new(OperationCancelled)),
                call_failure(&anyhow::Error::new(LeaseExpired)),
                call_failure(&name()),
                call_failure(&anyhow::Error::new(BlobRangeError { start: 3, end: 2 })),
                call_failure(&anyhow::Error::new(BlobMissingError {
                    path: Path::new("a").into()
                })),
                call_failure(&anyhow::Error::new(io::Error::from(
                    io::ErrorKind::PermissionDenied
                ))),
                call_failure(&elapsed().await),
                call_failure(&anyhow::anyhow!("the bucket is gone")),
                call_failure(&anyhow::Error::new(io::Error::from(
                    io::ErrorKind::StorageFull
                ))),
            ],
            [
                CallFailure::Cancelled,
                CallFailure::LeaseExpired,
                CallFailure::Permanent,
                CallFailure::Permanent,
                CallFailure::Permanent,
                CallFailure::Permanent,
                CallFailure::TimedOut,
                CallFailure::Failed,
                CallFailure::Failed,
            ]
        );
    }

    const EVERY_FAILURE: [CallFailure; 5] = [
        CallFailure::Cancelled,
        CallFailure::LeaseExpired,
        CallFailure::Permanent,
        CallFailure::TimedOut,
        CallFailure::Failed,
    ];

    #[test]
    fn a_write_can_still_land_after_every_failure_but_a_permanent_one() {
        assert_eq!(EVERY_FAILURE.map(may_land), [true, true, false, true, true]);
    }

    #[test]
    fn each_failure_of_a_last_try_ends_a_run_by_its_class() {
        assert_eq!(
            EVERY_FAILURE.map(run_end),
            [
                RunEnd::Cancelled,
                RunEnd::CallFailed,
                RunEnd::Permanent,
                RunEnd::CallFailed,
                RunEnd::CallFailed,
            ]
        );
    }

    #[test]
    async fn a_failed_call_ends_a_run_by_its_class() {
        assert_eq!(
            [
                storage_end(&anyhow::Error::new(OperationCancelled)),
                storage_end(&anyhow::Error::new(BlobRangeError { start: 3, end: 2 })),
                storage_end(&anyhow::Error::new(LeaseExpired)),
                storage_end(&elapsed().await),
                storage_end(&anyhow::anyhow!("the bucket is gone")),
            ],
            [
                RunEnd::Cancelled,
                RunEnd::Permanent,
                RunEnd::CallFailed,
                RunEnd::CallFailed,
                RunEnd::CallFailed,
            ]
        );
    }

    #[test]
    fn only_a_failed_blob_call_ends_a_run_of_rustic_as_a_storage_failure() {
        assert_eq!(
            [
                rustic_storage_end(&failed_call(anyhow::anyhow!("the bucket is gone"))),
                rustic_storage_end(&failed_call(anyhow::Error::new(OperationCancelled))),
                rustic_storage_end(&failed_call(anyhow::Error::new(BlobNameError::NoName {
                    path: std::path::PathBuf::new(),
                }))),
                rustic_storage_end(&rustic(io::Error::from(io::ErrorKind::PermissionDenied))),
                rustic_storage_end(&missing("index/ab12")),
            ],
            [
                Some(RunEnd::CallFailed),
                Some(RunEnd::Cancelled),
                Some(RunEnd::Permanent),
                None,
                None,
            ]
        );
    }

    #[test]
    fn a_missing_file_gives_its_path_and_its_directory() {
        let index = missing("index/ab12");
        let pack = missing("data/ab/ab12");

        assert_eq!(
            (
                missing_file_path(index.as_ref()),
                is_index_missing(index.as_ref()),
                is_pack_missing(index.as_ref()),
                missing_file_path(pack.as_ref()),
                is_index_missing(pack.as_ref()),
                is_pack_missing(pack.as_ref()),
                missing_file_path(anyhow::anyhow!("other").as_ref()),
            ),
            (
                Some(Path::new("index/ab12")),
                true,
                false,
                Some(Path::new("data/ab/ab12")),
                false,
                true,
                None,
            )
        );
    }

    #[test]
    fn missing_blob_reads_the_start_of_the_id_of_the_forks_not_found_errors() {
        let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .parse::<Id>()
            .unwrap();
        // The errors as the fork makes them: `get_index_entry` for a data blob, and the tree
        // load of the node streamer for a tree blob. The fork writes the context with the short
        // display of the id.
        let data = anyhow::Error::new(
            RusticError::new(
                ErrorKind::Internal,
                "Blob ID `{id}` not found in index, but should be there.",
            )
            .attach_context("id", id.to_string()),
        );
        let tree = anyhow::Error::new(
            RusticError::new(
                ErrorKind::Internal,
                "Tree ID `{tree_id}` not found in index",
            )
            .attach_context("tree_id", id.to_string()),
        )
        .context("ls the snapshot");
        let other = anyhow::Error::new(RusticError::new(
            ErrorKind::Internal,
            "the data failed its check",
        ));

        assert_eq!(
            [
                missing_blob(data.as_ref()),
                missing_blob(tree.as_ref()),
                missing_blob(other.as_ref()),
            ],
            [
                Some(Box::from("01234567")),
                Some(Box::from("01234567")),
                None
            ]
        );
    }

    #[test]
    fn missing_blob_reads_only_a_hex_start_of_one_to_64_digits() {
        let not_found = |id: &str| anyhow::anyhow!("Blob ID `{id}` not found in index");

        assert_eq!(
            [
                "",
                "xyz",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "ABCD",
            ]
            .map(|id| missing_blob(not_found(id).as_ref())),
            [
                None,
                None,
                None,
                Some(Box::from(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                )),
                Some(Box::from("abcd")),
            ]
        );
    }

    #[test]
    fn the_config_marker_is_found_in_the_chain() {
        let exists = failed_call(anyhow::Error::new(ConfigExists));
        let other = failed_call(anyhow::anyhow!("the bucket is gone"));

        assert_eq!(
            (
                is_config_exists(exists.as_ref()),
                is_config_exists(other.as_ref())
            ),
            (true, false)
        );
    }
}
