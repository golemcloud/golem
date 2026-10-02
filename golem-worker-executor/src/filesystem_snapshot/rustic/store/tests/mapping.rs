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

//! How the store reads the error of a failed step: the error of a restore in each phase, the
//! error of a save, and the id of a blob that the index of rustic does not hold.

use super::super::super::fault::{
    BlobCallFailed, FileMissing, OperationCancelled, Phase, RestoreError, missing_blob,
    restore_error,
};
use super::super::super::runs::{Ran, RunEnd};
use super::super::save_failure;
use super::*;
use crate::filesystem_snapshot::SnapshotInfo;
use golem_service_base::storage::blob::{BlobNameError, BlobRangeError};
use pretty_assertions::assert_eq;
use rustic_core::{DataId, ErrorKind, Id, RusticError, TreeId};
use std::io;
use test_r::{test, timeout};

/// Gives a rustic error whose source is the error, as rustic gives it to the store.
fn rustic(source: impl std::error::Error + Send + Sync + 'static) -> anyhow::Error {
    anyhow::Error::new(RusticError::with_source(
        ErrorKind::Backend,
        "the operation failed",
        source,
    ))
}

/// A blob call that failed with `failure`, as the backend gives it to rustic.
fn failed_call(failure: anyhow::Error) -> anyhow::Error {
    rustic(BlobCallFailed::new(failure))
}

fn missing(path: &str) -> anyhow::Error {
    rustic(FileMissing {
        path: Path::new(path).into(),
    })
}

/// An error that holds no storage failure and no I/O error.
fn refused() -> anyhow::Error {
    anyhow::Error::new(RusticError::new(
        ErrorKind::Cryptography,
        "the data failed its check",
    ))
}

/// The error below the rustic error of a restore that cannot set an extended attribute, as the
/// fork gives it on ext4 for a user attribute of 6,000 bytes.
#[derive(Debug)]
struct SettingXattrFailed(io::Error);

impl std::fmt::Display for SettingXattrFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "setting xattr `user.golem-test` on `\"/restore/file.txt\"` with `{:?}`",
            self.0
        )
    }
}

impl std::error::Error for SettingXattrFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// The cause of a local I/O error as the fork gives it: an error that shows the OS error in its
/// text and has no source.
#[derive(Debug)]
struct OpeningFileFailed;

impl std::fmt::Display for OpeningFileFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "opening file failed: `Os { code: 13, kind: PermissionDenied, message: \"Permission denied\" }`",
        )
    }
}

impl std::error::Error for OpeningFileFailed {}

/// The error of a write of a restore whose file cannot be opened, as the fork gives it.
fn local_io_without_source() -> anyhow::Error {
    anyhow::Error::new(RusticError::with_source(
        ErrorKind::InputOutput,
        "Failed to set the length of the file `{path}`. Please check the path and try again.",
        OpeningFileFailed,
    ))
}

const PACK: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn a_restore_error_is_read_from_the_storage_first_and_then_from_its_phase() {
    let each_phase = |error: fn() -> anyhow::Error| {
        [Phase::Load, Phase::Check, Phase::Write].map(|phase| restore_error(&error(), phase))
    };
    let pack = PACK.parse::<Id>().unwrap();

    assert_eq!(
        [
            each_phase(|| missing(&format!("data/01/{PACK}"))),
            each_phase(|| missing("data/01/not-an-id")),
            each_phase(|| missing("config")),
            each_phase(|| missing("index/0123")),
            each_phase(|| failed_call(anyhow::Error::new(OperationCancelled))),
            each_phase(|| failed_call(anyhow::Error::new(BlobRangeError { start: 4, end: 9 }))),
            each_phase(|| {
                failed_call(anyhow::Error::new(BlobNameError::NotUtf8 {
                    path: "x".into(),
                }))
            }),
            each_phase(|| failed_call(anyhow::anyhow!("the bucket gave 503"))),
            each_phase(|| rustic(io::Error::new(io::ErrorKind::StorageFull, "no space"))),
            each_phase(|| {
                anyhow::Error::new(RusticError::with_source(
                    ErrorKind::InputOutput,
                    "The restore cannot set the extended attributes of `file.txt`.",
                    SettingXattrFailed(io::Error::from_raw_os_error(28)),
                ))
            }),
            each_phase(local_io_without_source),
            each_phase(refused),
        ],
        [
            [(); 3].map(|()| RestoreError::PackMissing(Some(pack))),
            [
                RestoreError::PackMissing(None),
                RestoreError::PackMissing(None),
                RestoreError::PackMissing(None)
            ],
            [
                RestoreError::ConfigMissing,
                RestoreError::ConfigMissing,
                RestoreError::ConfigMissing
            ],
            [
                RestoreError::FileMissing(Path::new("index/0123").into()),
                RestoreError::FileMissing(Path::new("index/0123").into()),
                RestoreError::FileMissing(Path::new("index/0123").into()),
            ],
            [
                RestoreError::Cancelled,
                RestoreError::Cancelled,
                RestoreError::Cancelled
            ],
            [(); 3].map(|()| RestoreError::Permanent { range: true }),
            [(); 3].map(|()| RestoreError::Permanent { range: false }),
            [
                RestoreError::CallFailed,
                RestoreError::CallFailed,
                RestoreError::CallFailed
            ],
            [
                RestoreError::Corrupt,
                RestoreError::Destination(io::ErrorKind::StorageFull),
                RestoreError::Destination(io::ErrorKind::StorageFull),
            ],
            [
                RestoreError::Corrupt,
                RestoreError::Destination(io::ErrorKind::StorageFull),
                RestoreError::Destination(io::ErrorKind::StorageFull),
            ],
            [
                RestoreError::Corrupt,
                RestoreError::Destination(io::ErrorKind::PermissionDenied),
                RestoreError::Destination(io::ErrorKind::PermissionDenied),
            ],
            [
                RestoreError::Corrupt,
                RestoreError::CheckFailed,
                RestoreError::Corrupt
            ],
        ]
    );
}

/// Gives the end of a run of a save, or the kind of the error of the answer.
fn save_shape(ran: Ran<Result<SnapshotInfo, SaveError>>) -> String {
    match ran {
        Ran::Ended(ended) => format!("ended {:?}", ended.end),
        Ran::Answered(Err(SaveError::Source(error))) => format!("source {:?}", error.kind()),
        Ran::Answered(answer) | Ran::AnsweredAfter(answer, _) => format!("answered {answer:?}"),
    }
}

#[test]
fn a_save_error_ends_the_run_by_its_storage_and_answers_source_for_a_local_io_error() {
    assert_eq!(
        [
            failed_call(anyhow::anyhow!("the bucket gave 503")),
            failed_call(anyhow::Error::new(OperationCancelled)),
            failed_call(anyhow::Error::new(BlobNameError::NotUtf8 {
                path: "x".into()
            })),
            missing("index/0123"),
            rustic(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "a file of the tree"
            )),
            local_io_without_source(),
            refused(),
        ]
        .map(|error| save_shape(save_failure(error))),
        [
            format!("ended {:?}", RunEnd::CallFailed),
            format!("ended {:?}", RunEnd::Cancelled),
            format!("ended {:?}", RunEnd::Permanent),
            format!("ended {:?}", RunEnd::CallFailed),
            format!("source {:?}", io::ErrorKind::PermissionDenied),
            format!("source {:?}", io::ErrorKind::PermissionDenied),
            format!("ended {:?}", RunEnd::Permanent),
        ]
    );
}

#[test]
#[timeout("60s")]
async fn missing_blob_reads_the_start_of_the_id_of_each_lookup_error_that_the_fork_gives() {
    // The errors come from the fork itself: a lookup of a data blob and a load of a tree whose id
    // the index of a real repository does not hold.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("indexed");
    store
        .save(
            &scope,
            &name("p-indexed"),
            tree.path(),
            None,
            crate::filesystem_snapshot::never_cancelled(),
            &crate::filesystem_snapshot::Unlimited,
        )
        .await
        .unwrap();
    let backend = backend_of(storage, &scope, LONG_DEADLINE);
    let id = PACK.parse::<Id>().unwrap();

    let found = tokio::task::spawn_blocking(move || {
        let repository = open_existing(backend, &key())
            .unwrap()
            .unwrap()
            .to_indexed()
            .unwrap();
        let data = repository
            .get_index_entry(&DataId::from(id))
            .map(|_| ())
            .map_err(anyhow::Error::from)
            .unwrap_err();
        let tree = repository
            .get_tree(&TreeId::from(id))
            .map(|_| ())
            .map_err(anyhow::Error::from)
            .unwrap_err()
            .context("read the tree of the snapshot");
        [missing_blob(data.as_ref()), missing_blob(tree.as_ref())]
    })
    .await
    .unwrap();

    // The fork writes the short display of the id, its first 8 hex digits.
    let start = Some(Box::<str>::from(&PACK[..8]));
    assert_eq!(found, [start.clone(), start]);
}
