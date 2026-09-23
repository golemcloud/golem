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
//! failed operation as a [`SnapshotStoreError`].
//!
//! rustic gives no public kind of an error. So the classification reads the chain of sources: the
//! markers of this module, the name errors of the blob storage, and the I/O errors.

use crate::filesystem_snapshot::SnapshotStoreError;
use golem_service_base::storage::blob::BlobNameError;
use std::error::Error;
use std::fmt::{Display, Formatter};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileMissing;

impl Display for FileMissing {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the blob storage holds no file at the path")
    }
}

impl Error for FileMissing {}

/// Tells whether an error in the chain is [`FileMissing`].
pub(super) fn is_file_missing(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<FileMissing>())
}

/// Tells whether an error in the chain is [`ConfigExists`].
pub(super) fn is_config_exists(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<ConfigExists>())
}

/// The kind of operation that failed. It tells where an I/O error came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    /// Reads a tree from the local filesystem and writes it into the repository.
    Save,
    /// Reads the repository and writes a tree into the local filesystem.
    Restore,
    /// Reads or changes only the repository.
    Repository,
}

/// Gives the error of the store for an operation that failed with the error.
///
/// - A failed blob storage call gives `Storage`. It is retryable unless a name error of the blob
///   storage caused it, because a name error is the same for each new try.
/// - An I/O error gives `Source` in a save and `Destination` in a restore, with the kind of that
///   I/O error.
/// - Each other error gives `Storage` that is not retryable in a save, and `Corrupt` in the other
///   operations, because the repository gave data that rustic refused.
pub(super) fn classify(operation: Operation, error: anyhow::Error) -> SnapshotStoreError {
    let from_storage = chain(error.as_ref()).any(|error| error.is::<BlobCallFailed>());
    let io_kind = chain(error.as_ref())
        .find_map(|error| error.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind);
    let permanent = chain(error.as_ref()).any(|error| error.is::<BlobNameError>());
    match (from_storage, io_kind, operation) {
        (true, _, _) => SnapshotStoreError::Storage {
            retryable: !permanent,
            source: error,
        },
        (false, Some(kind), Operation::Save) => {
            SnapshotStoreError::Source(std::io::Error::new(kind, error_text(&error)))
        }
        (false, Some(kind), Operation::Restore) => {
            SnapshotStoreError::Destination(std::io::Error::new(kind, error_text(&error)))
        }
        (false, _, Operation::Save) => SnapshotStoreError::Storage {
            retryable: false,
            source: error,
        },
        (false, _, Operation::Restore | Operation::Repository) => {
            SnapshotStoreError::Corrupt(error)
        }
    }
}

/// Tells whether an error in the chain is a failed blob storage call.
pub(super) fn is_storage_failure(error: &(dyn Error + 'static)) -> bool {
    chain(error).any(|error| error.is::<BlobCallFailed>())
}

/// Gives the error of the store for a blob storage call that the store made without rustic. It is
/// retryable unless a name error of the blob storage caused it.
pub(super) fn storage_failure(error: anyhow::Error) -> SnapshotStoreError {
    classify(
        Operation::Repository,
        anyhow::Error::new(BlobCallFailed::new(error)),
    )
}

/// Gives the text of the error with the text of each of its sources.
fn error_text(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

/// Gives the error and each error in its chain of sources.
fn chain<'a>(error: &'a (dyn Error + 'static)) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    std::iter::successors(Some(error), |&error| error.source())
}

#[cfg(test)]
mod tests {
    use super::{
        BlobCallFailed, ConfigExists, Operation, OperationCancelled, classify, is_config_exists,
    };
    use crate::filesystem_snapshot::SnapshotStoreError;
    use golem_service_base::storage::blob::BlobNameError;
    use pretty_assertions::assert_eq;
    use rustic_core::{ErrorKind, RusticError};
    use std::io;
    use test_r::test;

    /// Gives a rustic error whose source is the error, as rustic gives it to the store.
    fn rustic(source: impl std::error::Error + Send + Sync + 'static) -> anyhow::Error {
        anyhow::Error::new(RusticError::with_source(
            ErrorKind::Backend,
            "the operation failed",
            source,
        ))
    }

    fn storage_failure(failure: anyhow::Error) -> anyhow::Error {
        rustic(BlobCallFailed::new(failure))
    }

    /// Gives the variant of the error, whether it is retryable, and the kind of its I/O error.
    fn shape(error: &SnapshotStoreError) -> (&'static str, Option<bool>, Option<io::ErrorKind>) {
        match error {
            SnapshotStoreError::NotFound => ("NotFound", None, None),
            SnapshotStoreError::AlreadyExists => ("AlreadyExists", None, None),
            SnapshotStoreError::Source(error) => ("Source", None, Some(error.kind())),
            SnapshotStoreError::Destination(error) => ("Destination", None, Some(error.kind())),
            SnapshotStoreError::Storage { retryable, .. } => ("Storage", Some(*retryable), None),
            SnapshotStoreError::Corrupt(_) => ("Corrupt", None, None),
        }
    }

    #[test]
    fn a_failed_blob_storage_call_gives_a_retryable_storage_error_in_each_operation() {
        let shapes =
            [Operation::Save, Operation::Restore, Operation::Repository].map(|operation| {
                shape(&classify(
                    operation,
                    storage_failure(anyhow::anyhow!("the bucket is gone")),
                ))
            });

        assert_eq!(shapes, [("Storage", Some(true), None); 3]);
    }

    #[test]
    fn a_blob_storage_call_that_holds_an_io_error_is_still_a_storage_error() {
        let failure = anyhow::Error::new(io::Error::new(io::ErrorKind::StorageFull, "no space"));

        assert_eq!(
            shape(&classify(Operation::Restore, storage_failure(failure))),
            ("Storage", Some(true), None)
        );
    }

    #[test]
    fn a_name_error_of_the_blob_storage_is_not_retryable() {
        let failure = anyhow::Error::new(BlobNameError::NoName {
            path: std::path::PathBuf::new(),
        });

        assert_eq!(
            shape(&classify(Operation::Repository, storage_failure(failure))),
            ("Storage", Some(false), None)
        );
    }

    #[test]
    fn a_cancelled_operation_gives_a_retryable_storage_error_in_each_operation() {
        let shapes =
            [Operation::Save, Operation::Restore, Operation::Repository].map(|operation| {
                shape(&classify(
                    operation,
                    storage_failure(anyhow::Error::new(OperationCancelled)),
                ))
            });

        assert_eq!(shapes, [("Storage", Some(true), None); 3]);
    }

    #[test]
    fn an_io_error_of_a_save_gives_source_with_its_kind() {
        let error = rustic(io::Error::new(io::ErrorKind::PermissionDenied, "locked"));

        let classified = classify(Operation::Save, error);

        assert_eq!(
            (
                shape(&classified),
                classified.to_string().contains("locked")
            ),
            (
                ("Source", None, Some(io::ErrorKind::PermissionDenied)),
                true
            )
        );
    }

    #[test]
    fn a_full_volume_during_a_restore_gives_destination() {
        let error = rustic(io::Error::new(io::ErrorKind::StorageFull, "no space"));

        assert_eq!(
            shape(&classify(Operation::Restore, error)),
            ("Destination", None, Some(io::ErrorKind::StorageFull))
        );
    }

    #[test]
    fn a_metadata_error_of_a_restore_gives_destination() {
        let error = rustic(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "setting extended attributes failed",
        ));

        assert_eq!(
            shape(&classify(Operation::Restore, error)),
            ("Destination", None, Some(io::ErrorKind::PermissionDenied))
        );
    }

    #[test]
    fn an_error_without_storage_or_io_is_corrupt_when_it_reads_and_not_retryable_in_a_save() {
        let refused = || {
            anyhow::Error::new(RusticError::new(
                ErrorKind::Cryptography,
                "the data failed its check",
            ))
        };

        assert_eq!(
            [
                shape(&classify(Operation::Restore, refused())),
                shape(&classify(Operation::Repository, refused())),
                shape(&classify(Operation::Save, refused())),
            ],
            [
                ("Corrupt", None, None),
                ("Corrupt", None, None),
                ("Storage", Some(false), None),
            ]
        );
    }

    #[test]
    fn the_config_marker_is_found_in_the_chain() {
        let exists = storage_failure(anyhow::Error::new(ConfigExists));
        let other = storage_failure(anyhow::anyhow!("the bucket is gone"));

        assert_eq!(
            (
                is_config_exists(exists.as_ref()),
                is_config_exists(other.as_ref())
            ),
            (true, false)
        );
    }
}
