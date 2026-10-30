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

use super::call_execution::{Ended, RunEnd};
use super::repository_layout::SNAPSHOTS_PATH;
use golem_service_base::storage::blob::{
    BlobFailure, BlobMissingError, BlobNameError, BlobRangeError,
};
use rustic_core::FileType;
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
    /// request that the storage refuses, or a blob that is not there.
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
    } else if in_chain(is_permanent) || is_refused(error) {
        CallFailure::Permanent
    } else if in_chain(|error| error.is::<tokio::time::error::Elapsed>()) {
        CallFailure::TimedOut
    } else {
        CallFailure::Failed
    }
}

/// Tells whether an error is an answer of the storage that no try can change. A refused request
/// gets the same answer until an operator fixes the storage.
fn is_permanent(error: &(dyn Error + 'static)) -> bool {
    error.is::<BlobNameError>()
        || error.is::<BlobRangeError>()
        || error.is::<BlobMissingError>()
        || error
            .downcast_ref::<BlobCallFailed>()
            .is_some_and(|failed| is_refused(&failed.failure))
        || error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
}

/// Tells whether the storage refused the request. A backend attaches the [`BlobRefusedError`] as a
/// context, so only the `anyhow` error finds it, and not the chain of sources.
fn is_refused(error: &anyhow::Error) -> bool {
    matches!(BlobFailure::of(error), BlobFailure::Refused(_))
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

/// Gives the path of the first [`FileMissing`] in the chain.
pub(super) fn missing_file_path<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a Path> {
    chain(error).find_map(|error| {
        error
            .downcast_ref::<FileMissing>()
            .map(|missing| &*missing.path)
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
pub(super) fn io_error_kind(error: &(dyn Error + 'static)) -> Option<std::io::ErrorKind> {
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

/// Gives the error and each error in its chain of sources.
pub(super) fn chain<'a>(
    error: &'a (dyn Error + 'static),
) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    std::iter::successors(Some(error), |&error| error.source())
}

/// Gives the end of a run whose step of rustic failed with `error`, when no other rule of the
/// operation applies: a failed storage call, a cancel, or an error that no new run changes.
pub(super) fn ended_by(error: anyhow::Error) -> Ended {
    Ended::new(
        rustic_storage_end(&error).unwrap_or(RunEnd::Permanent),
        error,
    )
}

#[cfg(test)]
mod tests;
