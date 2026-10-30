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

use super::{OperationCancelled, Phase, RestoreError, restore_error};
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    LIMIT, LONG_DEADLINE, NEVER, eventually, mapping_failed_call, mapping_local_io_without_source,
    mapping_missing, mapping_refused, mapping_rustic, name, policy, save_each, store, store_over,
};
use crate::filesystem_snapshot::test_support::fixture::Scratch;
use crate::filesystem_snapshot::test_support::new_scope;
use crate::filesystem_snapshot::{FilesystemSnapshotStore, RestoreFailure, Unlimited};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobNameError, BlobRangeError};
use pretty_assertions::assert_eq;
use rustic_core::{ErrorKind, Id, RusticError};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use test_r::{test, timeout};

#[test]
#[timeout("60s")]
async fn the_text_of_a_local_io_error_of_the_fork_keeps_the_form_that_the_store_reads() {
    // The store reads the kind of a local I/O error of the fork from its text: an error of the
    // kind `InputOutput` whose cause shows `Os { code: N`. A directory appears at the path of the
    // only file of the tree while the run loads the repository, so the write cannot open the file,
    // for each user. A fork whose text changes fails this test.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1"]).await;
    let first_read = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(inner, {
        let first_read = first_read.clone();
        move |op_label, path| {
            if op_label == "read"
                && path == Path::new("config")
                && !first_read.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let into = Scratch::new();
    let restored_name = name("p-1");
    let restoring = store.restore(&scope, &restored_name, into.path(), &Unlimited);
    tokio::pin!(restoring);
    let reached = tokio::select! {
        biased;
        reached = eventually(|| first_read.load(Ordering::SeqCst)) => reached,
        restored = &mut restoring => panic!("the restore ended before its load: {restored:?}"),
    };
    std::fs::create_dir(into.path().join("file.txt")).unwrap();
    storage.open_gate();
    let restored = tokio::time::timeout(LIMIT, restoring).await.unwrap();

    assert!(reached);
    match &restored {
        Err(RestoreFailure::Destination(error)) => {
            let text = error.to_string();
            assert_eq!(
                (
                    error.kind(),
                    text.starts_with(
                        "`rustic_core` experienced an error related to `input/output operations`."
                    ),
                    text.contains("Os { code: 21"),
                ),
                (std::io::ErrorKind::IsADirectory, true, true),
                "{text}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_restore_error_is_read_from_the_storage_first_and_then_from_its_phase() {
    let each_phase = |error: fn() -> anyhow::Error| {
        [Phase::Load, Phase::Check, Phase::Write].map(|phase| restore_error(&error(), phase))
    };
    let pack = PACK.parse::<Id>().unwrap();

    assert_eq!(
        [
            each_phase(|| mapping_missing(&format!("data/01/{PACK}"))),
            each_phase(|| mapping_missing("data/01/not-an-id")),
            each_phase(|| mapping_missing("config")),
            each_phase(|| mapping_missing("index/0123")),
            each_phase(|| mapping_failed_call(anyhow::Error::new(OperationCancelled))),
            each_phase(|| mapping_failed_call(anyhow::Error::new(BlobRangeError {
                start: 4,
                end: 9
            }))),
            each_phase(|| {
                mapping_failed_call(anyhow::Error::new(BlobNameError::NotUtf8 {
                    path: "x".into(),
                }))
            }),
            each_phase(|| mapping_failed_call(anyhow::anyhow!("the bucket gave 503"))),
            each_phase(|| mapping_rustic(io::Error::new(io::ErrorKind::StorageFull, "no space"))),
            each_phase(|| {
                anyhow::Error::new(RusticError::with_source(
                    ErrorKind::InputOutput,
                    "The restore cannot set the extended attributes of `file.txt`.",
                    SettingXattrFailed(io::Error::from_raw_os_error(28)),
                ))
            }),
            each_phase(mapping_local_io_without_source),
            each_phase(mapping_refused),
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

/// The error below the mapping_rustic error of a restore that cannot set an extended attribute, as the
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

const PACK: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

mod error_chains {

    use super::super::{
        BlobCallFailed, CallFailure, ConfigExists, FileMissing, LeaseExpired, OperationCancelled,
        call_failure, is_config_exists, is_index_missing, is_pack_missing, may_land,
        missing_file_path, run_end, rustic_storage_end, storage_end,
    };
    use crate::filesystem_snapshot::backend::rustic::call_execution::RunEnd;
    use golem_service_base::storage::blob::{
        BlobMissingError, BlobNameError, BlobRangeError, BlobRefusal, BlobRefusedError,
    };
    use pretty_assertions::assert_eq;
    use rustic_core::{ErrorKind, RusticError};
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
                call_failure(&anyhow::anyhow!("403 Forbidden").context(BlobRefusedError {
                    refusal: BlobRefusal::AccessDenied
                })),
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
                rustic_storage_end(&failed_call(anyhow::anyhow!("NoSuchBucket").context(
                    BlobRefusedError {
                        refusal: BlobRefusal::NoSuchBucket,
                    }
                ))),
                rustic_storage_end(&rustic(io::Error::from(io::ErrorKind::PermissionDenied))),
                rustic_storage_end(&missing("index/ab12")),
            ],
            [
                Some(RunEnd::CallFailed),
                Some(RunEnd::Cancelled),
                Some(RunEnd::Permanent),
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
