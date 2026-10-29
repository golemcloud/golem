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

#[cfg(unix)]
use super::super::running_as_root;
use super::*;
use crate::host_directories::HostDirectory;
use crate::host_directories::HostPath;
use crate::sandbox_filesystem::LinkGroup;
use crate::sandbox_filesystem::SandboxAccessMode;
use crate::sandbox_filesystem::SandboxFilesystemAdapter;
use crate::sandbox_filesystem::SandboxFollow;
use crate::sandbox_filesystem::SandboxObjectKind;
use crate::sandbox_filesystem::SandboxOpenOptions;
use crate::sandbox_filesystem::SeedAccess;
use crate::sandbox_filesystem::SeedEntry;
use crate::sandbox_filesystem::SeedPlacement;
use crate::sandbox_filesystem::errors::FilesystemStorageError;
use crate::sandbox_filesystem::host_adapter::NativeRoot;
use crate::sandbox_filesystem::host_adapter::SandboxFilesystem;
#[cfg(target_os = "macos")]
use crate::sandbox_filesystem::host_adapter::admit_root;
use crate::sandbox_filesystem::host_adapter::directory_for;
use crate::sandbox_filesystem::host_adapter::sandbox_provisioning::SandboxFilesystemName;
use crate::sandbox_filesystem::host_adapter::sandbox_provisioning::SandboxFilesystemProvisioning;
use crate::sandbox_filesystem::host_adapter::tree_copy;
#[cfg(test)]
use crate::sandbox_filesystem::host_adapter::tree_copy::CapabilityCopyParentProbe;
#[cfg(target_os = "macos")]
use crate::sandbox_filesystem::host_adapter::tree_copy::FileTransfer;
use crate::sandbox_filesystem::host_adapter::tree_copy::TreeExclusions;
use crate::sandbox_filesystem::object_identity::NativeFileIdentity;
use crate::sandbox_filesystem::open_handles::SandboxDirectory;
use crate::sandbox_filesystem::open_handles::SandboxHandle;
use crate::sandbox_filesystem::sandbox_paths::SandboxPath;
#[cfg(target_os = "linux")]
use crate::services::agent_filesystem::ExecutorDirectories;
use crate::services::golem_config::FilesystemStorageMode;
use crate::storage_volume::FileCopyMode;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::storage_volume::FilesystemSpace;
#[cfg(target_os = "macos")]
use crate::storage_volume::observe_space_blocking;
#[cfg(target_os = "linux")]
use crate::storage_volume::{
    AgentAccounting, FilesystemAllocation, FilesystemLimits, assign_project, cow_extent_size_hint,
    file_project_id, get_fsxattr, has_project_identity,
};
#[cfg(unix)]
use cap_std::fs::PermissionsExt as _;
use golem_common::model::RetryConfig;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::Write as _;
#[cfg(target_os = "macos")]
use std::io::{Seek as _, SeekFrom};
use std::num::NonZeroU32;
use std::num::NonZeroU64;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::UNIX_EPOCH;
use test_r::test;
#[cfg(target_os = "linux")]
use test_r::timeout;

#[test]
fn seed_file_submission_preserves_standard_order_and_finalizes_ordinary_failures() {
    use SeedFileSyncAction::{Ordinary, Standard};
    [
        (
            Standard,
            false,
            false,
            &["transfer", "sync", "attributes"][..],
        ),
        (
            Ordinary,
            false,
            false,
            &["transfer", "attributes", "sync"][..],
        ),
        (Standard, true, false, &["transfer"][..]),
        (Ordinary, true, false, &["transfer", "sync"][..]),
        (
            Standard,
            false,
            true,
            &["transfer", "sync", "attributes"][..],
        ),
        (
            Ordinary,
            false,
            true,
            &["transfer", "attributes", "sync"][..],
        ),
    ]
    .into_iter()
    .for_each(|(action, transfer_fails, attributes_fail, expected)| {
        let calls = std::cell::RefCell::new(Vec::new());
        let result = prepare_seed_file(
            action,
            || {
                calls.borrow_mut().push("transfer");
                if transfer_fails {
                    Err(std::io::Error::other("transfer failed"))
                } else {
                    Ok(())
                }
            },
            |_| {
                calls.borrow_mut().push("attributes");
                if attributes_fail {
                    Err(std::io::Error::other("attributes failed"))
                } else {
                    Ok(())
                }
            },
            || {
                calls.borrow_mut().push("sync");
                Ok(())
            },
        );
        assert_eq!(result.is_err(), transfer_fails || attributes_fail);
        assert_eq!(
            calls.borrow().as_slice(),
            expected,
            "{action:?} transfer failure={transfer_fails} attributes failure={attributes_fail}"
        );
    });
}

#[test]
fn seed_file_sync_failure_stops_standard_attributes_and_keeps_ordinary_attribute_errors() {
    use SeedFileSyncAction::{Ordinary, Standard};
    [
        (Standard, false, &["transfer", "sync"][..], "sync failed"),
        (
            Ordinary,
            false,
            &["transfer", "attributes", "sync"][..],
            "sync failed",
        ),
        (
            Ordinary,
            true,
            &["transfer", "attributes", "sync"][..],
            "attributes failed",
        ),
    ]
    .into_iter()
    .for_each(
        |(action, attributes_fail, expected_calls, expected_error)| {
            let calls = std::cell::RefCell::new(Vec::new());
            let result = prepare_seed_file(
                action,
                || {
                    calls.borrow_mut().push("transfer");
                    Ok(())
                },
                |_| {
                    calls.borrow_mut().push("attributes");
                    if attributes_fail {
                        Err(std::io::Error::other("attributes failed"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("sync");
                    Err(std::io::Error::other("sync failed"))
                },
            );
            assert_eq!(calls.borrow().as_slice(), expected_calls, "{action:?}");
            assert_eq!(
                result.unwrap_err().to_string(),
                expected_error,
                "{action:?}"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn ordinary_seed_file_sync_executes_the_selected_native_action() {
    use std::io::Write as _;
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"ordinary seed synchronization").unwrap();
    let capability_file = cap_std::fs::File::from_std(file.try_clone().unwrap());
    sync_seed_file(SeedFileSyncAction::Ordinary, &capability_file).unwrap();
    sync_ordinary_descriptor(&file).unwrap();
    assert_eq!(file.metadata().unwrap().len(), 29);
    assert_eq!(capability_file.metadata().unwrap().len(), 29);
}

#[cfg(not(unix))]
#[test]
fn ordinary_file_sync_is_unavailable_without_a_unix_descriptor() {
    let file = tempfile::tempfile().unwrap();
    assert_eq!(
        sync_ordinary_descriptor(&file).unwrap_err().kind(),
        std::io::ErrorKind::Unsupported
    );
    let capability_file = cap_std::fs::File::from_std(file);
    assert_eq!(
        sync_ordinary_descriptor(&capability_file)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
}

#[cfg(unix)]
#[test]
fn unsupported_seed_batch_actions_return_errors_without_panicking() {
    let directory = tempfile::tempdir().unwrap();
    // A root with a seed_flusher lets the macOS full flush reach its platform check.
    let root = NativeRoot::new(
        directory.path().to_path_buf(),
        File::open(directory.path()).unwrap(),
        SeedSyncPlatform::MacOs,
    )
    .unwrap();
    let admitted = root.directory.lock().unwrap().as_ref().unwrap().clone();
    let unavailable = SeedSyncPolicy::new(SeedSyncPlatform::Other, FileTransfer::Reflink);
    assert!(!unavailable.requires_batch_root());
    let error = sync_seed_batch(unavailable, None, Path::new("\0")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    #[cfg(target_os = "macos")]
    let wrong_platform = SeedSyncPolicy::new(SeedSyncPlatform::Linux, FileTransfer::Reflink);
    #[cfg(not(target_os = "macos"))]
    let wrong_platform = SeedSyncPolicy::new(SeedSyncPlatform::MacOs, FileTransfer::Bytes);
    let result = sync_seed_batch(wrong_platform, Some(&admitted), directory.path());
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
}

#[test]
fn seed_batch_requires_an_admitted_root_only_for_macos_fullflush() {
    let no_sync = SeedSyncPolicy::new(SeedSyncPlatform::Other, FileTransfer::Bytes);
    assert!(!no_sync.requires_batch_root());
    sync_seed_batch(no_sync, None, Path::new("\0")).unwrap();
    let full_flush = SeedSyncPolicy::new(SeedSyncPlatform::MacOs, FileTransfer::Bytes);
    assert!(full_flush.requires_batch_root());
    let error = sync_seed_batch(full_flush, None, Path::new("\0")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_volume_seed_batch_uses_the_root_path_without_root_admission() {
    let directory = tempfile::tempdir().unwrap();
    let policy = SeedSyncPolicy::new(SeedSyncPlatform::Linux, FileTransfer::Reflink);
    assert!(!policy.requires_batch_root());
    sync_seed_batch(policy, None, directory.path()).unwrap();
    let error = sync_seed_batch(policy, None, &directory.path().join("missing")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn platform_file_transfer_converts_only_macos_reflinks_to_clones() {
    use FileTransfer::{Bytes, Clone, Reflink, ReflinkIntoProject};
    use SeedSyncPlatform::{Linux, MacOs, Other};
    let project = ReflinkIntoProject(NonZeroU32::new(7).unwrap());
    [
        (MacOs, Bytes, Bytes),
        (MacOs, Clone, Clone),
        (MacOs, Reflink, Clone),
        (MacOs, project, project),
        (Linux, Bytes, Bytes),
        (Linux, Clone, Clone),
        (Linux, Reflink, Reflink),
        (Linux, project, project),
        (Other, Bytes, Bytes),
        (Other, Clone, Clone),
        (Other, Reflink, Reflink),
        (Other, project, project),
    ]
    .into_iter()
    .for_each(|(platform, transfer, expected)| {
        assert_eq!(
            platform_file_transfer(platform, transfer),
            expected,
            "{platform:?} {transfer:?}"
        );
    });
}

/// The seed policy on `platform` of a sandbox whose files get `transfer` before
/// [`platform_file_transfer`] applies the rule of the platform.
fn seed_policy(platform: SeedSyncPlatform, transfer: FileTransfer) -> SeedSyncPolicy {
    SeedSyncPolicy::new(platform, platform_file_transfer(platform, transfer))
}

#[test]
fn seed_file_sync_actions_use_ordinary_fsync_on_macos_and_standard_sync_elsewhere() {
    use SeedFileSyncAction::{Ordinary, Standard};
    use SeedSyncPlatform::{Linux, MacOs, Other};
    let project = FileTransfer::ReflinkIntoProject(NonZeroU32::new(7).unwrap());
    [
        (MacOs, FileTransfer::Bytes, Ordinary),
        (MacOs, FileTransfer::Clone, Ordinary),
        (MacOs, FileTransfer::Reflink, Ordinary),
        (MacOs, project, Standard),
        (Linux, FileTransfer::Bytes, Standard),
        (Linux, FileTransfer::Clone, Standard),
        (Linux, FileTransfer::Reflink, Standard),
        (Linux, project, Standard),
        (Other, FileTransfer::Bytes, Standard),
        (Other, FileTransfer::Clone, Standard),
        (Other, FileTransfer::Reflink, Standard),
        (Other, project, Standard),
    ]
    .into_iter()
    .for_each(|(platform, transfer, expected)| {
        assert_eq!(
            seed_policy(platform, transfer).file,
            expected,
            "{platform:?} {transfer:?}"
        );
    });
}

#[test]
fn seed_batch_actions_fullflush_macos_and_preserve_other_platform_policies() {
    use SeedBatchAction::{LinuxVolume, MacFullFlush, None as NoSync, Unavailable};
    use SeedSyncPlatform::{Linux, MacOs, Other};
    let project = FileTransfer::ReflinkIntoProject(NonZeroU32::new(7).unwrap());
    [
        (MacOs, FileTransfer::Bytes, MacFullFlush),
        (MacOs, FileTransfer::Clone, MacFullFlush),
        (MacOs, FileTransfer::Reflink, MacFullFlush),
        (MacOs, project, NoSync),
        (Linux, FileTransfer::Bytes, NoSync),
        (Linux, FileTransfer::Clone, NoSync),
        (Linux, FileTransfer::Reflink, LinuxVolume),
        (Linux, project, LinuxVolume),
        (Other, FileTransfer::Bytes, NoSync),
        (Other, FileTransfer::Clone, NoSync),
        (Other, FileTransfer::Reflink, Unavailable),
        (Other, project, Unavailable),
    ]
    .into_iter()
    .for_each(|(platform, transfer, expected)| {
        let policy = seed_policy(platform, transfer);
        assert_eq!(policy.batch, expected, "{platform:?} {transfer:?}");
        assert_eq!(
            policy.requires_batch_root(),
            expected == MacFullFlush,
            "{platform:?} {transfer:?}"
        );
    });
}

#[test]
fn seed_directory_actions_follow_the_platform_transfer_and_observed_stage() {
    use DirectoryPlacementObservation::{Failed, Made, Merged, NotAttempted};
    use SeedDirectoryAction::{ChildThenParent, None as NoSync, OpenedDirectory, Parent};
    use SeedDirectoryStage::{Ancestor, DirectoryAttributes, DirectoryPlacement, Entry};
    use SeedSyncPlatform::{Linux, MacOs, Other};
    let project = FileTransfer::ReflinkIntoProject(NonZeroU32::new(7).unwrap());
    let stages = [
        Entry,
        DirectoryPlacement(Made),
        DirectoryPlacement(Merged),
        DirectoryPlacement(Failed),
        Ancestor { opened: true },
        Ancestor { opened: false },
        DirectoryAttributes,
        DirectoryPlacement(NotAttempted),
    ];
    [
        (
            MacOs,
            FileTransfer::Reflink,
            [
                Parent,
                ChildThenParent,
                NoSync,
                Parent,
                ChildThenParent,
                Parent,
                OpenedDirectory,
                Parent,
            ],
        ),
        (
            MacOs,
            FileTransfer::Bytes,
            [
                Parent,
                ChildThenParent,
                NoSync,
                Parent,
                ChildThenParent,
                Parent,
                OpenedDirectory,
                Parent,
            ],
        ),
        (
            MacOs,
            FileTransfer::Clone,
            [
                Parent,
                ChildThenParent,
                NoSync,
                Parent,
                ChildThenParent,
                Parent,
                OpenedDirectory,
                Parent,
            ],
        ),
        (MacOs, project, [NoSync; 8]),
        (Linux, FileTransfer::Reflink, [NoSync; 8]),
        (Linux, FileTransfer::Clone, [NoSync; 8]),
        (Linux, project, [NoSync; 8]),
        (Linux, FileTransfer::Bytes, [NoSync; 8]),
        (Other, FileTransfer::Bytes, [NoSync; 8]),
        (Other, FileTransfer::Clone, [NoSync; 8]),
        (Other, FileTransfer::Reflink, [NoSync; 8]),
        (Other, project, [NoSync; 8]),
    ]
    .into_iter()
    .for_each(|(platform, transfer, expected)| {
        let policy = seed_policy(platform, transfer);
        assert_eq!(
            stages.map(|stage| policy.directory_action(stage)),
            expected,
            "{platform:?} {transfer:?}"
        );
    });
}

#[test]
fn seed_directory_finalizers_attempt_the_parent_after_a_child_failure() {
    let attempts = std::cell::RefCell::new(Vec::new());
    let result = sync_seed_directories(
        SeedDirectoryAction::ChildThenParent,
        || {
            attempts.borrow_mut().push("child");
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "child sync",
            ))
        },
        || {
            attempts.borrow_mut().push("parent");
            Err(std::io::Error::other("parent sync"))
        },
    );
    assert_eq!(*attempts.borrow(), ["child", "parent"]);
    let error = result.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "child sync");
}

#[test]
fn seed_directory_finalizers_do_only_the_selected_directory_work() {
    [
        (SeedDirectoryAction::None, &[][..]),
        (SeedDirectoryAction::Parent, &["parent"][..]),
        (SeedDirectoryAction::OpenedDirectory, &["opened"][..]),
        (
            SeedDirectoryAction::ChildThenParent,
            &["opened", "parent"][..],
        ),
    ]
    .into_iter()
    .for_each(|(action, expected)| {
        let attempts = std::cell::RefCell::new(Vec::new());
        sync_seed_directories(
            action,
            || {
                attempts.borrow_mut().push("opened");
                Ok(())
            },
            || {
                attempts.borrow_mut().push("parent");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*attempts.borrow(), expected, "{action:?}");
    });
}

#[test]
fn operation_finalization_keeps_the_first_error_and_runs_cleanup_before_sync() {
    ["transfer", "attributes", "file flush", "placement"]
        .into_iter()
        .for_each(|stage| {
            let attempts = std::cell::RefCell::new(Vec::new());
            let seeded = Err::<(), _>(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                stage,
            ));
            let cleaned = finalize_operation(
                || seeded,
                || {
                    attempts.borrow_mut().push("cleanup");
                    Err(std::io::Error::other("cleanup"))
                },
            );
            let result = finalize_operation(
                || cleaned,
                || {
                    attempts.borrow_mut().push("parent sync");
                    Err(std::io::Error::other("parent sync"))
                },
            );
            assert_eq!(*attempts.borrow(), ["cleanup", "parent sync"]);
            let error = result.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), stage);
        });
    let result = finalize_operation(|| Ok(17), || Err(std::io::Error::other("cleanup")));
    assert_eq!(result.unwrap_err().to_string(), "cleanup");
    let result = finalize_operation(|| Ok(17), || Err(std::io::Error::other("sync")));
    assert_eq!(result.unwrap_err().to_string(), "sync");
    assert_eq!(
        finalize_operation::<_, std::io::Error>(|| Ok(17), || Ok(())).unwrap(),
        17
    );
}

#[test]
fn seed_batch_finalization_runs_after_failure_and_keeps_the_seed_error() {
    let synced = std::cell::Cell::new(false);
    let result = finalize_operation(
        || {
            Err::<(), _>(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "seed",
            ))
        },
        || {
            synced.set(true);
            Err(std::io::Error::other("volume sync"))
        },
    );
    assert!(synced.get());
    assert_eq!(result.unwrap_err().to_string(), "seed");
}

#[test]
fn batch_submissions_attempt_every_stage_and_keep_the_earliest_error() {
    [
        "entry",
        "file submission",
        "root submission",
        "full flush",
        "success",
    ]
    .into_iter()
    .for_each(|failure| {
        let attempts = std::cell::RefCell::new(Vec::new());
        let stage = |name| {
            attempts.borrow_mut().push(name);
            if name == failure {
                Err(std::io::Error::other(name))
            } else {
                Ok(())
            }
        };
        let result = finalize_operation(
            || stage("entry"),
            || {
                sync_seed_batch_submissions(
                    || stage("file submission"),
                    || stage("root submission"),
                    || stage("full flush"),
                )
            },
        );
        assert_eq!(
            *attempts.borrow(),
            ["entry", "file submission", "root submission", "full flush"]
        );
        if failure == "success" {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().to_string(), failure);
        }
    });
    let result = finalize_operation(
        || Err::<(), _>(std::io::Error::other("entry")),
        || {
            sync_seed_batch_submissions(
                || Err(std::io::Error::other("file submission")),
                || Err(std::io::Error::other("root submission")),
                || Err(std::io::Error::other("full flush")),
            )
        },
    );
    assert_eq!(result.unwrap_err().to_string(), "entry");
}

#[test]
fn an_earlier_operation_error_survives_a_direct_finalizer_panic() {
    let attempts = std::cell::RefCell::new(Vec::new());
    let finalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finalize_operation(
            || {
                finalize_operation(
                    || {
                        finalize_operation(
                            || Err::<(), _>(std::io::Error::other("earlier operation error")),
                            || {
                                attempts.borrow_mut().push("cleanup");
                                panic!("later cleanup panic")
                            },
                        )
                    },
                    || {
                        attempts.borrow_mut().push("parent sync");
                        Ok(())
                    },
                )
            },
            || {
                sync_seed_batch_submissions(
                    || {
                        attempts.borrow_mut().push("file submission");
                        Ok(())
                    },
                    || {
                        attempts.borrow_mut().push("root submission");
                        Ok(())
                    },
                    || {
                        attempts.borrow_mut().push("full flush");
                        Ok(())
                    },
                )
            },
        )
    }));
    assert_eq!(
        *attempts.borrow(),
        [
            "cleanup",
            "parent sync",
            "file submission",
            "root submission",
            "full flush"
        ]
    );
    assert!(
        finalized.is_ok(),
        "a later finalizer panic must not replace the earlier ordinary error"
    );
    assert_eq!(
        finalized.unwrap().unwrap_err().to_string(),
        "earlier operation error"
    );
}

#[test]
fn an_earlier_error_survives_later_panics_and_all_outer_finalizers_run() {
    let attempts = std::cell::RefCell::new(Vec::new());
    let result = finalize_operation(
        || {
            finalize_operation(
                || {
                    finalize_operation(
                        || {
                            attempts.borrow_mut().push("operation");
                            Err::<(), _>(std::io::Error::other("earlier operation error"))
                        },
                        || {
                            attempts.borrow_mut().push("cleanup");
                            panic!("later cleanup panic")
                        },
                    )
                },
                || {
                    attempts.borrow_mut().push("parent sync");
                    Err(std::io::Error::other("parent sync"))
                },
            )
        },
        || {
            sync_seed_batch_submissions(
                || {
                    attempts.borrow_mut().push("file submission");
                    Err(std::io::Error::other("file submission"))
                },
                || {
                    attempts.borrow_mut().push("root submission");
                    panic!("later root submission panic")
                },
                || {
                    attempts.borrow_mut().push("full flush");
                    panic!("later full flush panic")
                },
            )
        },
    );
    assert_eq!(result.unwrap_err().to_string(), "earlier operation error");
    assert_eq!(
        *attempts.borrow(),
        [
            "operation",
            "cleanup",
            "parent sync",
            "file submission",
            "root submission",
            "full flush"
        ]
    );
}

#[test]
fn symlink_cleanup_actions_follow_observed_placement() {
    assert_eq!(
        seed_symlink_cleanup_action(false),
        SeedSymlinkCleanupAction::RemoveTemporary
    );
    assert_eq!(
        seed_symlink_cleanup_action(true),
        SeedSymlinkCleanupAction::None
    );
}

#[test]
fn unwind_runs_local_and_batch_finalizers_before_resuming_the_original_panic() {
    let attempts = std::cell::RefCell::new(Vec::new());
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finalize_operation(
            || {
                finalize_operation(
                    || {
                        finalize_operation(
                            || {
                                finalize_operation(
                                    || -> std::io::Result<()> {
                                        attempts.borrow_mut().push("transfer");
                                        panic!("original transfer panic")
                                    },
                                    || {
                                        attempts.borrow_mut().push("file sync");
                                        Ok(())
                                    },
                                )
                            },
                            || {
                                attempts.borrow_mut().push("cleanup");
                                panic!("cleanup panic")
                            },
                        )
                    },
                    || {
                        attempts.borrow_mut().push("parent sync");
                        Err(std::io::Error::other("parent sync"))
                    },
                )
            },
            || {
                sync_seed_batch_submissions(
                    || {
                        attempts.borrow_mut().push("file submission");
                        panic!("submission panic")
                    },
                    || {
                        attempts.borrow_mut().push("root submission");
                        Ok(())
                    },
                    || {
                        attempts.borrow_mut().push("full flush");
                        Ok(())
                    },
                )
            },
        )
    }))
    .unwrap_err();
    assert_eq!(
        panicked.downcast_ref::<&str>(),
        Some(&"original transfer panic")
    );
    assert_eq!(
        *attempts.borrow(),
        [
            "transfer",
            "file sync",
            "cleanup",
            "parent sync",
            "file submission",
            "root submission",
            "full flush"
        ]
    );
}

fn paths(items: &[&str]) -> HashSet<Box<Path>> {
    items
        .iter()
        .map(|item| Box::from(Path::new(item)))
        .collect()
}

fn exclusions(items: &[&str]) -> TreeExclusions {
    TreeExclusions::new(items.iter().map(PathBuf::from))
}

fn open(path: &Path) -> cap_std::fs::Dir {
    cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap()
}

fn buffered_seed(placement: SeedPlacement) -> SeedContext {
    SeedContext {
        policy: SeedSyncPolicy::new(native_seed_sync_platform(), FileTransfer::Bytes),
        access: SeedAccess::FromSource,
        placement,
    }
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn a_supplied_linux_reflink_policy_does_not_execute_a_macos_clone() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let file = source.path().join("file").into_boxed_path();
    std::fs::write(&file, b"selected transfer").unwrap();
    let directory = open(target.path());
    let result = seed_entry(
        SeedContext {
            policy: SeedSyncPolicy::new(SeedSyncPlatform::Linux, FileTransfer::Reflink),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        },
        &directory,
        &file,
        Path::new("file"),
    );
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    assert!(!target.path().join("file").exists());
    assert_eq!(directory.entries().unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn a_supplied_linux_capture_reflink_transfer_does_not_execute_a_macos_clone() {
    let source = tempfile::tempfile().unwrap();
    std::io::Write::write_all(&mut &source, b"selected capture transfer").unwrap();
    let destination = tempfile::tempdir().unwrap();
    let target = destination.path().join("file").into_boxed_path();
    let transfer = platform_file_transfer(SeedSyncPlatform::Linux, FileTransfer::Reflink);
    assert_eq!(transfer, FileTransfer::Reflink);
    let error = copy_new_file(transfer, &source, &target).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    assert_eq!(std::fs::metadata(&target).unwrap().len(), 0);
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
}

/// The rules of the `SeedPlacement` and `SeedEntry` docs, written as a table: the source, what
/// is at the target, the action with `CreateNew`, and the action with `Replace`.
const PLACEMENT_TABLE: [(&str, Occupant, PlacementAction, PlacementAction); 6] = [
    (
        "directory",
        Occupant::Directory,
        PlacementAction::Merge,
        PlacementAction::Merge,
    ),
    (
        "directory",
        Occupant::Other,
        PlacementAction::Refuse,
        PlacementAction::Overwrite,
    ),
    (
        "file",
        Occupant::Directory,
        PlacementAction::Refuse,
        PlacementAction::RemoveDirectory,
    ),
    (
        "file",
        Occupant::Other,
        PlacementAction::Refuse,
        PlacementAction::Overwrite,
    ),
    (
        "symlink",
        Occupant::Directory,
        PlacementAction::Refuse,
        PlacementAction::RemoveDirectory,
    ),
    (
        "symlink",
        Occupant::Other,
        PlacementAction::Refuse,
        PlacementAction::Overwrite,
    ),
];

fn source_kind(name: &str) -> TreeEntryKind {
    match name {
        "directory" => TreeEntryKind::Directory,
        "file" => TreeEntryKind::File,
        _ => TreeEntryKind::Symlink(Box::from(Path::new("target"))),
    }
}

#[test]
fn placement_action_follows_the_seed_placement_table() {
    PLACEMENT_TABLE
        .into_iter()
        .for_each(|(source, occupant, create_new, replace)| {
            [
                (SeedPlacement::CreateNew, create_new),
                (SeedPlacement::Replace, replace),
            ]
            .into_iter()
            .for_each(|(placement, expected)| {
                assert_eq!(
                    placement_action(&source_kind(source), occupant, placement),
                    expected,
                    "a {source} source on {occupant:?} with {placement:?}"
                );
            });
        });
}

fn listed(relative: &str, link: Option<&str>) -> TreeEntry {
    TreeEntry {
        relative: Box::from(Path::new(relative)),
        kind: TreeEntryKind::File,
        permissions: cap_std::fs::Permissions::from_std(
            std::fs::metadata(std::env::temp_dir())
                .unwrap()
                .permissions(),
        ),
        modified: None,
        link: link.map(|identity| NativeFileIdentity::Scripted(identity.to_string())),
    }
}

/// The names that a copy with `plan` makes, in the order of the listing.
fn copied_names<'a>(plan: &LinkPlan, entries: &'a [TreeEntry]) -> Vec<&'a str> {
    entries
        .iter()
        .enumerate()
        .filter(|(position, _)| plan.copies(*position))
        .map(|(_, entry)| entry.relative.to_str().unwrap())
        .collect()
}

fn group(first: &str, others: &[&str]) -> LinkGroup {
    LinkGroup {
        first: Box::from(Path::new(first)),
        others: others
            .iter()
            .map(|other| Box::from(Path::new(other)))
            .collect(),
    }
}

#[test]
fn link_plan_copies_each_object_once_at_its_first_name_and_groups_the_others() {
    let entries = [
        listed("a", None),
        listed("b1", Some("b")),
        listed("c1", Some("c")),
        listed("b2", Some("b")),
        listed("d", Some("d")),
        listed("e", None),
        listed("c2", Some("c")),
        listed("b3", Some("b")),
    ];

    let plan = link_plan(&entries);

    assert_eq!(copied_names(&plan, &entries), ["a", "b1", "c1", "d", "e"]);
    assert_eq!(plan.later, Box::from([3, 6, 7]));
    assert_eq!(
        plan.groups,
        Box::from([group("b1", &["b2", "b3"]), group("c1", &["c2"])])
    );
}

#[test]
fn link_plan_of_a_listing_without_links_copies_everything_and_gives_no_group() {
    let entries = [listed("a", None), listed("b", None)];

    let plan = link_plan(&entries);

    assert_eq!(copied_names(&plan, &entries), ["a", "b"]);
    assert!(plan.later.is_empty());
    assert!(plan.groups.is_empty());
}

#[cfg(unix)]
fn fixture_tree(root: &Path) {
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::create_dir(root.join("static")).unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::write(root.join("data/db.sqlite"), vec![0x5a; 12_000]).unwrap();
    std::fs::write(root.join("data/nested/note.txt"), b"nested note").unwrap();
    std::fs::write(root.join("static/asset.bin"), b"asset").unwrap();
    std::fs::write(root.join("config.toml"), b"[config]").unwrap();
    std::fs::write(root.join("script.sh"), b"#!/bin/sh").unwrap();
    std::fs::set_permissions(
        root.join("script.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::set_permissions(root.join("data"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::os::unix::fs::symlink("data/db.sqlite", root.join("db-link")).unwrap();
    std::os::unix::fs::symlink("/absolute/outside", root.join("outside-link")).unwrap();
    [
        ("data/db.sqlite", 1_700_000_001u64),
        ("data/nested/note.txt", 1_700_000_002),
        ("data/nested", 1_700_000_003),
        ("data", 1_700_000_004),
        ("script.sh", 1_700_000_005),
    ]
    .into_iter()
    .for_each(|(relative, seconds)| {
        let time = UNIX_EPOCH + Duration::from_secs(seconds);
        File::options()
            .write(true)
            .open(root.join(relative))
            .or_else(|_| File::open(root.join(relative)))
            .unwrap()
            .set_modified(time)
            .unwrap();
    });
}

#[test]
fn exclusions_keep_only_paths_that_can_name_an_entry() {
    let excluded = exclusions(&[
        "/lib/data.txt",
        "./config.toml",
        "a//b",
        "../escape",
        "a/../b",
        "/",
        ".",
        "",
    ]);

    assert_eq!(
        excluded.paths(),
        &paths(&["lib/data.txt", "config.toml", "a/b"])
    );
}

#[test]
fn exclusions_keep_a_root_relative_path_as_given() {
    let given = ["lib/data.txt", "lib//separator.txt", "directory/"];

    let excluded = exclusions(&given);

    let mut kept = excluded
        .paths()
        .iter()
        .map(|path| path.as_os_str().to_owned())
        .collect::<Vec<_>>();
    kept.sort();
    let mut expected = given.map(std::ffi::OsString::from).to_vec();
    expected.sort();
    assert_eq!(kept, expected);
    assert!(excluded.contains(Path::new("lib/data.txt")));
    assert!(excluded.contains(Path::new("lib/separator.txt")));
    assert!(excluded.contains(Path::new("directory")));
}

#[cfg(unix)]
#[test]
fn listing_skips_excluded_paths_and_their_contents() {
    let source = tempfile::tempdir().unwrap();
    fixture_tree(source.path());

    let entries = list_tree(
        &open(source.path()),
        &exclusions(&["/static/asset.bin", "data/nested", "missing"]),
    )
    .unwrap();

    let relatives = entries
        .iter()
        .map(|entry| entry.relative.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        relatives,
        [
            "config.toml",
            "data",
            "data/db.sqlite",
            "db-link",
            "empty",
            "outside-link",
            "script.sh",
            "static",
        ]
    );
    assert_eq!(
        entries[3].kind,
        TreeEntryKind::Symlink(Path::new("data/db.sqlite").into())
    );
    assert_eq!(
        entries[5].kind,
        TreeEntryKind::Symlink(Path::new("/absolute/outside").into())
    );
    assert_eq!(entries[6].permissions.mode() & 0o777, 0o755);
    assert_eq!(
        entries[2].modified,
        Some(UNIX_EPOCH + Duration::from_secs(1_700_000_001))
    );
}

#[cfg(unix)]
#[test]
fn listing_refuses_a_file_that_is_not_regular() {
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("regular"), b"contents").unwrap();
    let fifo =
        std::ffi::CString::new(source.path().join("pipe").into_os_string().into_vec()).unwrap();
    // SAFETY: `fifo` is a valid NUL-terminated path that lives for the whole call.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let destination = tempfile::tempdir().unwrap();

    let error = list_tree(&open(source.path()), &TreeExclusions::default()).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("pipe"), "{error}");
    let error = copy_contents(
        &open(source.path()),
        Path::new(""),
        &TreeExclusions::default(),
        destination.path(),
        FileTransfer::Bytes,
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        std::fs::read_dir(destination.path())
            .unwrap()
            .next()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn listing_does_not_follow_a_symlink_to_a_directory() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("real")).unwrap();
    std::fs::write(source.path().join("real/file"), b"contents").unwrap();
    std::os::unix::fs::symlink("real", source.path().join("alias")).unwrap();

    let entries = list_tree(&open(source.path()), &TreeExclusions::default()).unwrap();

    let relatives = entries
        .iter()
        .map(|entry| entry.relative.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(relatives, ["alias", "real", "real/file"]);
    assert_eq!(
        entries[0].kind,
        TreeEntryKind::Symlink(Path::new("real").into())
    );
}

#[cfg(unix)]
#[test]
fn copy_contents_copies_everything_except_the_exclusions() {
    let source = tempfile::tempdir().unwrap();
    fixture_tree(source.path());
    std::fs::set_permissions(source.path(), std::fs::Permissions::from_mode(0o750)).unwrap();
    let root_modified = UNIX_EPOCH + Duration::from_secs(1_700_000_006);
    File::open(source.path())
        .unwrap()
        .set_modified(root_modified)
        .unwrap();
    let destination = tempfile::tempdir().unwrap();

    copy_contents(
        &open(source.path()),
        Path::new(""),
        &exclusions(&["static/asset.bin", "data/nested"]),
        destination.path(),
        FileTransfer::Bytes,
    )
    .unwrap();

    let root = std::fs::metadata(destination.path()).unwrap();
    assert_eq!(root.permissions().mode() & 0o777, 0o750);
    assert_eq!(root.modified().unwrap(), root_modified);
    let mut expected = tree_listing(source.path());
    ["static/asset.bin", "data/nested", "data/nested/note.txt"]
        .into_iter()
        .for_each(|absent| {
            assert!(
                expected.remove(absent),
                "{absent} must be in the source listing"
            );
        });
    assert_eq!(tree_listing(destination.path()), expected);
    assert_eq!(
        std::fs::read(destination.path().join("data/db.sqlite")).unwrap(),
        vec![0x5a; 12_000]
    );
    assert_eq!(
        std::fs::read_link(destination.path().join("db-link")).unwrap(),
        PathBuf::from("data/db.sqlite")
    );
    assert_eq!(
        std::fs::read_link(destination.path().join("outside-link")).unwrap(),
        PathBuf::from("/absolute/outside")
    );
    let script = std::fs::metadata(destination.path().join("script.sh")).unwrap();
    assert_eq!(script.permissions().mode() & 0o777, 0o755);
    assert_eq!(
        script.modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_005)
    );
    let data = std::fs::metadata(destination.path().join("data")).unwrap();
    assert_eq!(data.permissions().mode() & 0o777, 0o750);
    assert_eq!(
        data.modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_004)
    );
    assert_eq!(
        std::fs::metadata(destination.path().join("data/db.sqlite"))
            .unwrap()
            .modified()
            .unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_001)
    );
}

#[cfg(unix)]
#[test]
fn seed_recreates_the_tree_through_the_capability() {
    let source = tempfile::tempdir().unwrap();
    fixture_tree(source.path());
    let destination = tempfile::tempdir().unwrap();

    seed_entry(
        buffered_seed(SeedPlacement::CreateNew),
        &open(destination.path()),
        source.path(),
        Path::new(""),
    )
    .unwrap();

    assert_eq!(
        tree_listing(destination.path()),
        tree_listing(source.path())
    );
    assert_eq!(
        std::fs::read(destination.path().join("data/nested/note.txt")).unwrap(),
        b"nested note"
    );
    assert_eq!(
        std::fs::read_link(destination.path().join("outside-link")).unwrap(),
        PathBuf::from("/absolute/outside")
    );
    let script = std::fs::metadata(destination.path().join("script.sh")).unwrap();
    assert_eq!(script.permissions().mode() & 0o777, 0o755);
    assert_eq!(
        script.modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_005)
    );
    let nested = std::fs::metadata(destination.path().join("data/nested")).unwrap();
    assert_eq!(
        nested.modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_003)
    );
    assert_eq!(
        std::fs::metadata(destination.path().join("data"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o750
    );
}

#[cfg(unix)]
#[test]
fn seed_replaces_a_file_with_a_directory_that_has_the_source_attributes() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("data")).unwrap();
    std::fs::write(source.path().join("data/file"), b"new").unwrap();
    std::fs::set_permissions(
        source.path().join("data"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    let modified = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    File::open(source.path().join("data"))
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let destination = tempfile::tempdir().unwrap();
    std::fs::write(destination.path().join("data"), b"old file").unwrap();

    seed_entry(
        buffered_seed(SeedPlacement::Replace),
        &open(destination.path()),
        source.path(),
        Path::new(""),
    )
    .unwrap();

    let made = std::fs::metadata(destination.path().join("data")).unwrap();
    assert!(made.is_dir(), "the file must give way to the directory");
    assert_eq!(
        made.permissions().mode() & 0o777,
        0o750,
        "the directory that takes the place of a file must have the mode of its source"
    );
    assert_eq!(
        made.modified().unwrap(),
        modified,
        "the directory that takes the place of a file must have the time of its source"
    );
    assert_eq!(
        std::fs::read(destination.path().join("data/file")).unwrap(),
        b"new"
    );
}

#[cfg(unix)]
#[test]
fn seed_files_preserve_attributes_and_cleanup_after_placement_failure() {
    [
        FileTransfer::Bytes,
        #[cfg(target_os = "macos")]
        FileTransfer::Reflink,
    ]
    .into_iter()
    .for_each(|transfer| {
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("file").into_boxed_path();
        std::fs::write(&file, b"seeded file").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_007);
        File::open(&file).unwrap().set_modified(modified).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let base = open(destination.path());
        let context = SeedContext {
            policy: seed_policy(native_seed_sync_platform(), transfer),
            access: SeedAccess::ReadOnly,
            placement: SeedPlacement::CreateNew,
        };

        seed_entry(context, &base, &file, Path::new("ancestor/file")).unwrap();

        let target = destination.path().join("ancestor/file").into_boxed_path();
        let attributes = std::fs::metadata(&target).unwrap();
        assert_eq!(attributes.permissions().mode() & 0o777, 0o440);
        assert_eq!(attributes.modified().unwrap(), modified);
        [
            ("ancestor/file", std::io::ErrorKind::AlreadyExists),
            ("ancestor/\0", std::io::ErrorKind::InvalidInput),
        ]
        .into_iter()
        .for_each(|(path, expected)| {
            let error = seed_entry(context, &base, &file, Path::new(path)).unwrap_err();
            assert_eq!(error.kind(), expected, "{transfer:?}: {error}");
            assert_eq!(std::fs::read(&target).unwrap(), b"seeded file");
            assert_eq!(
                sorted_names(&base.open_dir_nofollow("ancestor").unwrap()).unwrap(),
                [OsString::from("file")],
                "a failed placement must remove its temporary file"
            );
        });
    });
}

#[cfg(unix)]
#[test]
fn seed_symlinks_preserve_times_and_cleanup_after_placement_failure() {
    [
        FileTransfer::Bytes,
        #[cfg(target_os = "macos")]
        FileTransfer::Reflink,
    ]
    .into_iter()
    .for_each(|transfer| {
        let source = tempfile::tempdir().unwrap();
        let link = source.path().join("link").into_boxed_path();
        std::os::unix::fs::symlink("missing-target", &link).unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_008);
        open(source.path())
            .set_symlink_times("link", None, Some(capability_time(modified)))
            .unwrap();
        let destination = tempfile::tempdir().unwrap();
        let base = open(destination.path());
        let context = SeedContext {
            policy: seed_policy(native_seed_sync_platform(), transfer),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        };

        seed_entry(context, &base, &link, Path::new("ancestor/link")).unwrap();
        let error = seed_entry(context, &base, &link, Path::new("ancestor/link")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        let replacing = SeedContext {
            placement: SeedPlacement::Replace,
            ..context
        };
        let error = seed_entry(replacing, &base, &link, Path::new("ancestor/\0")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        seed_entry(replacing, &base, &link, Path::new("ancestor/link")).unwrap();

        let target = destination.path().join("ancestor/link").into_boxed_path();
        assert_eq!(
            std::fs::read_link(&target).unwrap(),
            Path::new("missing-target")
        );
        assert_eq!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        assert_eq!(
            sorted_names(&base.open_dir_nofollow("ancestor").unwrap()).unwrap(),
            [OsString::from("link")],
            "placement and error cleanup must leave no temporary symlink"
        );
    });
}

#[cfg(unix)]
#[test]
fn seed_merges_an_empty_directory_without_opening_it_for_sync() {
    [
        FileTransfer::Bytes,
        #[cfg(target_os = "macos")]
        FileTransfer::Reflink,
    ]
    .into_iter()
    .for_each(|transfer| {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let base = open(destination.path());
        base.create_dir("merged").unwrap();
        base.set_permissions(
            "merged",
            cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o0)),
        )
        .unwrap();
        let mut entry = listed("merged", None);
        entry.kind = TreeEntryKind::Directory;
        let context = SeedContext {
            policy: seed_policy(native_seed_sync_platform(), transfer),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        };

        let result = seed_listed_entry(context, &open(source.path()), &base, &entry, Vec::new());
        std::fs::set_permissions(
            destination.path().join("merged"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();

        assert!(result.unwrap().is_empty(), "a merged directory is not made");
    });
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn macos_seed_syncs_directory_attributes_through_restrictive_permissions() {
    [FileTransfer::Bytes, FileTransfer::Reflink]
        .into_iter()
        .for_each(|transfer| {
            let destination = tempfile::tempdir().unwrap();
            let base = open(destination.path());
            base.create_dir("made").unwrap();
            let mut entry = listed("made", None);
            entry.kind = TreeEntryKind::Directory;
            entry.permissions =
                cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o0));
            let modified = UNIX_EPOCH + Duration::from_secs(1_234_567);
            entry.modified = Some(modified);
            let result = set_seeded_directory_attributes(
                seed_policy(SeedSyncPlatform::MacOs, transfer),
                &base,
                Path::new("made"),
                &entry,
            );
            let mode = base.symlink_metadata("made").unwrap().permissions().mode() & 0o777;
            std::fs::set_permissions(
                destination.path().join("made"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();

            assert!(
                result.is_ok(),
                "sync must use the open directory: {result:?}"
            );
            assert_eq!(mode, 0);
            assert_eq!(
                base.symlink_metadata("made")
                    .unwrap()
                    .modified()
                    .unwrap()
                    .into_std(),
                modified
            );
        });
}

#[test]
fn seed_fails_on_an_existing_target_path() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("data")).unwrap();
    std::fs::write(source.path().join("data/file"), b"new").unwrap();
    let destination = tempfile::tempdir().unwrap();
    std::fs::create_dir(destination.path().join("data")).unwrap();
    std::fs::write(destination.path().join("data/file"), b"old").unwrap();

    let error = seed_entry(
        buffered_seed(SeedPlacement::CreateNew),
        &open(destination.path()),
        source.path(),
        Path::new(""),
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read(destination.path().join("data/file")).unwrap(),
        b"old"
    );
}

#[cfg(unix)]
#[test]
fn copy_contents_copies_a_symlink_with_several_names_once_and_reports_its_names() {
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("target"), b"target").unwrap();
    std::os::unix::fs::symlink("target", source.path().join("link")).unwrap();
    std::fs::hard_link(source.path().join("link"), source.path().join("link-name")).unwrap();
    let destination = tempfile::tempdir().unwrap();

    let groups = copy_contents(
        &open(source.path()),
        Path::new(""),
        &exclusions(&[]),
        destination.path(),
        FileTransfer::Bytes,
    )
    .unwrap();

    assert_eq!(
        groups.as_ref(),
        [LinkGroup {
            first: Path::new("link").into(),
            others: Box::new([Box::from(Path::new("link-name"))]),
        }]
    );
    assert_eq!(
        tree_listing(destination.path()),
        ["link", "target"].map(String::from).into()
    );
    assert_eq!(
        std::fs::read_link(destination.path().join("link")).unwrap(),
        Path::new("target")
    );
}

#[test]
fn copy_contents_copies_a_file_with_several_names_once_and_reports_its_names() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("a")).unwrap();
    std::fs::create_dir(source.path().join("b")).unwrap();
    std::fs::write(source.path().join("a/file"), b"linked").unwrap();
    std::fs::hard_link(source.path().join("a/file"), source.path().join("b/second")).unwrap();
    std::fs::hard_link(source.path().join("a/file"), source.path().join("third")).unwrap();
    std::fs::write(source.path().join("single"), b"single").unwrap();
    std::fs::write(source.path().join("excluded"), b"excluded").unwrap();
    std::fs::hard_link(source.path().join("excluded"), source.path().join("kept")).unwrap();
    let destination = tempfile::tempdir().unwrap();

    let groups = copy_contents(
        &open(source.path()),
        Path::new(""),
        &exclusions(&["excluded"]),
        destination.path(),
        FileTransfer::Bytes,
    )
    .unwrap();

    assert_eq!(
        groups.as_ref(),
        [LinkGroup {
            first: Path::new("a/file").into(),
            others: [Path::new("b/second"), Path::new("third")]
                .map(Box::<Path>::from)
                .into(),
        }]
    );
    assert_eq!(
        tree_listing(destination.path()),
        ["a", "a/file", "b", "kept", "single"]
            .map(String::from)
            .into()
    );
    assert_eq!(
        std::fs::read(destination.path().join("a/file")).unwrap(),
        b"linked"
    );
    assert_eq!(
        std::fs::read(destination.path().join("kept")).unwrap(),
        b"excluded"
    );
}

#[test]
fn a_panicking_temporary_write_removes_its_name_before_resuming_unwind() {
    let root = tempfile::tempdir().unwrap();
    let directory =
        cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
    let temporary = CapabilityTempFile::new(&directory).unwrap();
    let name = temporary.name.as_ref().unwrap().clone();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        temporary.finish::<()>(|_| panic!("seed write panic"))
    }))
    .unwrap_err();
    assert_eq!(panicked.downcast_ref::<&str>(), Some(&"seed write panic"));
    assert!(!directory.try_exists(name).unwrap());
}

#[cfg(unix)]
#[test]
fn a_failed_temporary_unlink_retains_its_name_for_explicit_cleanup() {
    use std::os::unix::fs::PermissionsExt as _;
    if running_as_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let directory =
        cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
    let mut temporary = CapabilityTempFile::new(&directory).unwrap();
    let name = temporary.name.as_ref().unwrap().clone();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = temporary.cleanup();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        failed.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(temporary.name.as_deref(), Some(name.as_ref()));
    assert!(directory.try_exists(&name).unwrap());
    temporary.cleanup().unwrap();
    assert!(temporary.name.is_none());
    assert!(!directory.try_exists(&name).unwrap());
}

#[test]
fn a_panicking_clone_open_attempts_removal_before_resuming_unwind() {
    let removed = std::cell::Cell::new(false);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_cloned_file(
            || panic!("clone open panic"),
            || {
                removed.set(true);
                Err(std::io::Error::other("clone cleanup"))
            },
        )
    }))
    .unwrap_err();
    assert!(removed.get());
    assert_eq!(panicked.downcast_ref::<&str>(), Some(&"clone open panic"));
}

#[test]
fn a_clone_open_error_precedes_a_temporary_removal_error() {
    let removed = std::cell::Cell::new(false);
    let result = open_cloned_file(
        || {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "open cloned file",
            ))
        },
        || {
            removed.set(true);
            Err(std::io::Error::other("remove cloned file"))
        },
    );

    assert!(removed.get(), "the failed clone open must attempt cleanup");
    let error = result.expect_err("opening the clone failed");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "open cloned file");
}

async fn assert_pinned_seed_after_directory_rename(
    parent: &Path,
    provisioning: SandboxFilesystemProvisioning,
) {
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::create_dir(root.join("pinned")).unwrap();
    let pinned = open_sandbox_directory(&filesystem, "pinned").await;
    std::fs::rename(root.join("pinned"), root.join("renamed")).unwrap();
    std::fs::create_dir(root.join("pinned")).unwrap();

    let host_source = create_scratch_directory(parent).await;
    std::fs::write(host_source.path().as_path().join("source"), b"seeded").unwrap();
    let source = host_child_path(&host_source, "source");
    seed_one_entry(
        &filesystem,
        SeedEntry {
            source: source.clone(),
            target: SandboxPath::at(pinned.clone(), "destination"),
            access: SeedAccess::ReadOnly,
            placement: SeedPlacement::CreateNew,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(root.join("renamed/destination")).unwrap(),
        b"seeded"
    );
    assert!(
        std::fs::metadata(root.join("renamed/destination"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert!(!root.join("pinned/destination").exists());
    let existing = seed_one_entry(
        &filesystem,
        SeedEntry {
            source,
            target: SandboxPath::at(pinned, "destination"),
            access: SeedAccess::ReadOnly,
            placement: SeedPlacement::CreateNew,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

fn host_child_path(directory: &HostDirectory, name: &str) -> HostPath {
    directory.path().child(std::ffi::OsStr::new(name)).unwrap()
}

/// Makes the host directories under `root` on development storage, and gives `.scratch`.
async fn create_scratch_directory(root: &Path) -> HostDirectory {
    crate::services::agent_filesystem::provision_executor_directories(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .await
    .unwrap()
    .1
    .scratch
}

#[cfg(target_os = "macos")]
fn macos_sandbox_provisionings(parent: &Path) -> Box<[SandboxFilesystemProvisioning]> {
    [
        FilesystemStorageMode::Temporary,
        FilesystemStorageMode::Directory {
            root: parent.join("directory-agents").into_boxed_path(),
        },
        FilesystemStorageMode::Apfs {
            root: parent.join("apfs-agents").into_boxed_path(),
        },
    ]
    .iter()
    .map(|storage| SandboxFilesystemProvisioning::new(storage, RetryConfig::default()).unwrap())
    .collect()
}

#[cfg(unix)]
fn permissions_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

fn sandbox_name() -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "environment".to_string(),
        "component".to_string(),
        "filesystem".to_string(),
    )
    .unwrap()
}

async fn open_sandbox_directory(
    filesystem: &SandboxFilesystem,
    path: impl Into<PathBuf>,
) -> SandboxDirectory {
    match filesystem
        .open(
            SandboxPath::at_root(path),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::Directory,
                access: SandboxAccessMode::ReadWrite,
                follow: SandboxFollow::Yes,
            },
        )
        .await
        .unwrap()
    {
        SandboxHandle::Directory(directory) => directory,
        SandboxHandle::File(_) => panic!("path must open as a directory"),
    }
}

fn make_seed_entry(
    source: HostPath,
    target: impl Into<PathBuf>,
    access: SeedAccess,
    placement: SeedPlacement,
) -> SeedEntry {
    SeedEntry {
        source,
        target: SandboxPath::at_root(target),
        access,
        placement,
    }
}

async fn seed_one_entry(
    filesystem: &SandboxFilesystem,
    entry: SeedEntry,
) -> Result<(), FilesystemStorageError> {
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(filesystem, Box::new([entry])).await
}

fn directory_sandbox_provisioning(root: PathBuf) -> SandboxFilesystemProvisioning {
    SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Directory { root: root.into() },
        RetryConfig::default(),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
async fn copy_contents_copies_what_is_under_the_source_minus_the_exclusions() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let copies = create_scratch_directory(parent.path()).await;
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/nested/note"), b"note").unwrap();
    std::fs::write(root.join("data/db"), b"db").unwrap();
    std::fs::write(root.join("static.bin"), b"static").unwrap();
    std::fs::set_permissions(root.join("data/db"), std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("data/db", root.join("link")).unwrap();
    let whole = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("whole"))
        .await
        .unwrap();
    let part = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("part"))
        .await
        .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::new([
            PathBuf::from("static.bin"),
            PathBuf::from("data/nested"),
        ])),
        whole.path(),
    )
    .await
    .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root("data"),
        Arc::new(TreeExclusions::new([PathBuf::from("db")])),
        part.path(),
    )
    .await
    .unwrap();

    let whole_root = whole.path().as_path();
    assert_eq!(
        tree_copy::tree_listing(whole_root),
        std::collections::BTreeSet::from(["data", "data/db", "link"].map(String::from))
    );
    assert_eq!(std::fs::read(whole_root.join("data/db")).unwrap(), b"db");
    assert_eq!(permissions_mode(&whole_root.join("data/db")), 0o640);
    assert_eq!(
        std::fs::read_link(whole_root.join("link")).unwrap(),
        PathBuf::from("data/db")
    );
    assert_eq!(
        tree_copy::tree_listing(part.path().as_path()),
        std::collections::BTreeSet::from(["nested", "nested/note"].map(String::from))
    );
    assert_eq!(
        std::fs::read(root.join("static.bin")).unwrap(),
        b"static",
        "the sandbox must stay as it is"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn copy_contents_refuses_a_target_that_is_not_an_empty_directory() {
    let parent = tempfile::tempdir().unwrap();
    let copies = create_scratch_directory(parent.path()).await;
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    std::fs::write(filesystem.root().join("file"), b"file").unwrap();
    let copies_root = copies.path().as_path().to_path_buf();
    std::fs::create_dir(copies_root.join("full")).unwrap();
    std::fs::write(copies_root.join("full/kept"), b"kept").unwrap();
    std::fs::write(copies_root.join("plain-file"), b"plain").unwrap();
    std::fs::create_dir(copies_root.join("real")).unwrap();
    std::os::unix::fs::symlink("real", copies_root.join("alias")).unwrap();

    let results = futures::future::join_all(
        [
            ("full", std::io::ErrorKind::DirectoryNotEmpty),
            ("absent", std::io::ErrorKind::NotFound),
            ("plain-file", std::io::ErrorKind::NotADirectory),
            ("alias", std::io::ErrorKind::NotADirectory),
        ]
        .into_iter()
        .map(|(name, expected)| {
            let target = host_child_path(&copies, name);
            let filesystem = &filesystem;
            async move {
                let result = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
                    filesystem,
                    SandboxPath::at_root(""),
                    Arc::new(TreeExclusions::default()),
                    &target,
                )
                .await;
                (name, expected, result)
            }
        }),
    )
    .await;

    results.into_iter().for_each(|(name, expected, result)| {
        assert_eq!(
            result.unwrap_err().io_kind(),
            Some(expected),
            "a copy into {name} must be refused"
        );
    });
    assert_eq!(
        tree_copy::tree_listing(&copies_root),
        std::collections::BTreeSet::from(
            ["alias", "full", "full/kept", "plain-file", "real"].map(String::from)
        )
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
async fn an_empty_standard_seed_succeeds_after_root_retirement() {
    let parent = tempfile::tempdir().unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    filesystem.root.close();
    filesystem.seed(Box::default()).await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_empty_seed_preserves_read_only_root_namespace_and_mtime() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    futures::future::join_all(
        macos_sandbox_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| async move {
                let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
                let root = filesystem.root().to_path_buf().into_boxed_path();
                let modified =
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_100);
                File::open(&root).unwrap().set_modified(modified).unwrap();
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
                let result = filesystem.seed(Box::new([])).await;
                let attributes = std::fs::metadata(&root).unwrap();
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
                result.unwrap();
                assert_eq!(attributes.permissions().mode() & 0o777, 0);
                assert_eq!(attributes.modified().unwrap(), modified);
                assert!(std::fs::read_dir(&root).unwrap().next().is_none());
                SandboxFilesystem::delete_and_verify(filesystem)
                    .await
                    .unwrap();
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_seed_uses_a_writable_child_capability_under_an_unsearchable_root() {
    use cap_std::fs::PermissionsExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let source = host_child_path(&sources, "file");
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_101);
    std::fs::write(source.as_path(), b"seeded through child").unwrap();
    std::fs::set_permissions(source.as_path(), std::fs::Permissions::from_mode(0o640)).unwrap();
    File::open(source.as_path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    futures::future::join_all(
        macos_sandbox_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let source = source.clone();
                async move {
                    let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
                    let root = filesystem.root().to_path_buf().into_boxed_path();
                    std::fs::create_dir(root.join("child")).unwrap();
                    let child = open_sandbox_directory(&filesystem, "child").await;
                    File::open(&root).unwrap().set_modified(modified).unwrap();
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                    let result = filesystem
                        .seed(Box::new([SeedEntry {
                            source,
                            target: SandboxPath::at(child.clone(), "file"),
                            access: SeedAccess::ReadOnly,
                            placement: SeedPlacement::CreateNew,
                        }]))
                        .await;
                    let root_attributes = std::fs::metadata(&root).unwrap();
                    let child_attributes = child.host().unwrap().metadata("file");
                    let contents = child.host().unwrap().read("file");
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    result.unwrap();
                    let child_attributes = child_attributes.unwrap();
                    assert_eq!(child_attributes.permissions().mode() & 0o777, 0o440);
                    assert_eq!(child_attributes.modified().unwrap().into_std(), modified);
                    assert_eq!(contents.unwrap(), b"seeded through child");
                    assert_eq!(root_attributes.permissions().mode() & 0o777, 0);
                    assert_eq!(root_attributes.modified().unwrap(), modified);
                    assert_eq!(
                        tree_copy::tree_listing(&root),
                        ["child", "child/file"].map(String::from).into()
                    );
                    drop(child);
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_seed_and_batch_flush_follow_the_pinned_root_after_path_replacement() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let source = host_child_path(&sources, "file");
    std::fs::write(source.as_path(), b"pinned root content").unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_102);
    File::open(source.as_path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    futures::future::join_all(
        macos_sandbox_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let source = source.clone();
                async move {
                    let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
                    let root = filesystem.root().to_path_buf().into_boxed_path();
                    let moved = root.with_extension("moved").into_boxed_path();
                    std::fs::rename(&root, &moved).unwrap();
                    std::fs::create_dir(&root).unwrap();
                    std::fs::write(root.join("decoy"), b"unchanged").unwrap();
                    File::open(&root).unwrap().set_modified(modified).unwrap();
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                    let result = seed_one_entry(
                        &filesystem,
                        make_seed_entry(
                            source,
                            "file",
                            SeedAccess::ReadOnly,
                            SeedPlacement::CreateNew,
                        ),
                    )
                    .await;
                    let replacement_attributes = std::fs::metadata(&root).unwrap();
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    let replacement_listing = tree_copy::tree_listing(&root);
                    let seeded_content = std::fs::read(moved.join("file"));
                    let seeded_attributes = std::fs::metadata(moved.join("file"));
                    std::fs::remove_dir_all(&root).unwrap();
                    std::fs::rename(&moved, &root).unwrap();
                    result.unwrap();
                    assert_eq!(replacement_attributes.modified().unwrap(), modified);
                    assert_eq!(replacement_listing, ["decoy"].map(String::from).into());
                    assert_eq!(seeded_content.unwrap(), b"pinned root content");
                    let seeded_attributes = seeded_attributes.unwrap();
                    assert_eq!(seeded_attributes.modified().unwrap(), modified);
                    assert_eq!(seeded_attributes.permissions().mode() & 0o222, 0);
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_seed_flushes_directory_only_and_symlink_only_batches() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let tree = host_child_path(&sources, "tree");
    std::fs::create_dir_all(tree.as_path().join("nested/empty")).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_103);
    File::open(tree.as_path())
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let link = host_child_path(&sources, "link");
    std::os::unix::fs::symlink("missing", link.as_path()).unwrap();
    futures::future::join_all(
        macos_sandbox_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let tree = tree.clone();
                let link = link.clone();
                async move {
                    let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
                    seed_one_entry(
                        &filesystem,
                        make_seed_entry(
                            tree,
                            "tree",
                            SeedAccess::FromSource,
                            SeedPlacement::CreateNew,
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        std::fs::metadata(filesystem.root().join("tree"))
                            .unwrap()
                            .modified()
                            .unwrap(),
                        modified
                    );
                    seed_one_entry(
                        &filesystem,
                        make_seed_entry(
                            link,
                            "link",
                            SeedAccess::FromSource,
                            SeedPlacement::CreateNew,
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        std::fs::read_link(filesystem.root().join("link")).unwrap(),
                        Path::new("missing")
                    );
                    assert_eq!(
                        tree_copy::tree_listing(filesystem.root()),
                        ["link", "tree", "tree/nested", "tree/nested/empty"]
                            .map(String::from)
                            .into()
                    );
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn macos_partial_seed_keeps_prior_entries_and_removes_temporary_names() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let source = host_child_path(&sources, "file");
    std::fs::write(source.as_path(), b"prior entry").unwrap();
    let link = host_child_path(&sources, "link");
    std::os::unix::fs::symlink("file", link.as_path()).unwrap();
    futures::future::join_all(
        macos_sandbox_provisionings(parent.path())
            .into_vec()
            .into_iter()
            .map(|provisioning| {
                let source = source.clone();
                let link = link.clone();
                async move {
                    let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
                    let result = filesystem
                        .seed(Box::new([
                            make_seed_entry(
                                source.clone(),
                                "ancestor/file",
                                SeedAccess::ReadOnly,
                                SeedPlacement::CreateNew,
                            ),
                            make_seed_entry(
                                link,
                                "ancestor/link",
                                SeedAccess::FromSource,
                                SeedPlacement::CreateNew,
                            ),
                            make_seed_entry(
                                source.clone(),
                                "ancestor/file",
                                SeedAccess::ReadOnly,
                                SeedPlacement::CreateNew,
                            ),
                            make_seed_entry(
                                source,
                                "not-visited",
                                SeedAccess::FromSource,
                                SeedPlacement::CreateNew,
                            ),
                        ]))
                        .await;
                    assert_eq!(
                        result.unwrap_err().io_kind(),
                        Some(std::io::ErrorKind::AlreadyExists)
                    );
                    assert_eq!(
                        std::fs::read(filesystem.root().join("ancestor/file")).unwrap(),
                        b"prior entry"
                    );
                    assert_eq!(
                        std::fs::read_link(filesystem.root().join("ancestor/link")).unwrap(),
                        Path::new("file")
                    );
                    assert_eq!(
                        tree_copy::tree_listing(filesystem.root()),
                        ["ancestor", "ancestor/file", "ancestor/link"]
                            .map(String::from)
                            .into()
                    );
                    SandboxFilesystem::delete_and_verify(filesystem)
                        .await
                        .unwrap();
                }
            }),
    )
    .await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn apfs_seed_clones_unflushed_source_content_without_changing_its_attributes() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let source = host_child_path(&sources, "unflushed");
    let contents = (0..128_000)
        .map(|position| (position % 251) as u8)
        .collect::<Box<[u8]>>();
    let mut file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(source.as_path())
        .unwrap();
    file.write_all(&contents).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_104);
    file.set_modified(modified).unwrap();
    let before = file.metadata().unwrap();
    let provisioning = SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::Apfs {
            root: parent.path().join("apfs-agents").into_boxed_path(),
        },
        RetryConfig::default(),
    )
    .unwrap();
    let filesystem = provisioning.create_fresh(sandbox_name()).await.unwrap();
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            source,
            "clone",
            SeedAccess::ReadOnly,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(filesystem.root().join("clone")).unwrap(),
        contents.as_ref()
    );
    assert_eq!(
        std::fs::metadata(filesystem.root().join("clone"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    let after = file.metadata().unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.permissions(), before.permissions());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_puts_entries_in_order_and_stops_at_the_first_failure() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    std::fs::write(sources.path().as_path().join("file"), b"first").unwrap();
    std::fs::write(sources.path().as_path().join("after"), b"after").unwrap();
    std::os::unix::fs::symlink("file", sources.path().as_path().join("link")).unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();

    let error = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            make_seed_entry(
                host_child_path(&sources, "file"),
                "nested/file",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "link"),
                "nested/link",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "after"),
                "nested/file",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "after"),
                "after",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap_err();

    let root = filesystem.root();
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert!(
        error.to_string().contains("nested/file"),
        "the error must name the target of the failed entry: {error}"
    );
    assert_eq!(std::fs::read(root.join("nested/file")).unwrap(), b"first");
    assert_eq!(
        std::fs::read_link(root.join("nested/link")).unwrap(),
        PathBuf::from("file")
    );
    assert!(
        !root.join("after").exists(),
        "an entry after the failed entry must not be seeded"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_access_sets_the_write_permission_of_seeded_files() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_010);
    [
        ("tool", 0o755),
        ("locked", 0o555),
        ("private", 0o640),
        ("writable", 0o644),
    ]
    .into_iter()
    .for_each(|(name, mode)| {
        let path = sources.path().as_path().join(name);
        std::fs::write(&path, name).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    });
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            make_seed_entry(
                host_child_path(&sources, "tool"),
                "tool",
                SeedAccess::ReadOnly,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "locked"),
                "locked",
                SeedAccess::ReadWrite,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "private"),
                "private",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "writable"),
                "writable",
                SeedAccess::ReadWrite,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap();

    let root = filesystem.root();
    [
        ("tool", 0o555),
        ("locked", 0o755),
        ("private", 0o640),
        ("writable", 0o644),
    ]
    .into_iter()
    .for_each(|(name, expected)| {
        assert_eq!(
            permissions_mode(&root.join(name)),
            expected,
            "{name} must have mode {expected:o}"
        );
        assert_eq!(
            std::fs::metadata(root.join(name))
                .unwrap()
                .modified()
                .unwrap(),
            modified,
            "{name} must have the modification time of its source"
        );
    });
    assert_eq!(std::fs::read(root.join("tool")).unwrap(), b"tool");
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_gives_a_made_target_directory_the_source_attributes_and_keeps_a_merged_one() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let tree = sources.path().as_path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("file"), b"file").unwrap();
    std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o750)).unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    File::open(&tree).unwrap().set_modified(modified).unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    std::fs::create_dir(root.join("merged")).unwrap();
    std::fs::set_permissions(root.join("merged"), std::fs::Permissions::from_mode(0o700)).unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            make_seed_entry(
                host_child_path(&sources, "tree"),
                "made",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "tree"),
                "merged",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap();

    assert_eq!(
        permissions_mode(&root.join("made")),
        0o750,
        "the made directory must have the mode of its source"
    );
    assert_eq!(
        std::fs::metadata(root.join("made"))
            .unwrap()
            .modified()
            .unwrap(),
        modified,
        "the made directory must have the modification time of its source"
    );
    assert_eq!(
        permissions_mode(&root.join("merged")),
        0o700,
        "the merged directory must keep its own mode"
    );
    assert_eq!(std::fs::read(root.join("merged/file")).unwrap(), b"file");
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_reports_the_error_of_a_directory_that_it_cannot_make() {
    use std::os::unix::fs::PermissionsExt as _;

    if running_as_root() {
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    std::fs::create_dir(sources.path().as_path().join("tree")).unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let locked = filesystem.root().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let error = seed_one_entry(
        &filesystem,
        make_seed_entry(
            host_child_path(&sources, "tree"),
            "locked/new",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        error.io_kind(),
        Some(std::io::ErrorKind::PermissionDenied),
        "{error}"
    );
    assert!(!locked.join("new").exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_file_entries_follow_the_placement() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    std::fs::write(sources.path().as_path().join("new"), b"new").unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    ["create-new", "replace"].into_iter().for_each(|name| {
        std::fs::write(root.join(name), b"old").unwrap();
    });
    std::fs::create_dir(root.join("directory")).unwrap();
    std::fs::write(root.join("directory/child"), b"child").unwrap();
    let new = || host_child_path(&sources, "new");

    let failed = seed_one_entry(
        &filesystem,
        make_seed_entry(
            new(),
            "create-new",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            new(),
            "absent",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            new(),
            "replace",
            SeedAccess::ReadOnly,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();
    let failed_over_directory = seed_one_entry(
        &filesystem,
        make_seed_entry(
            new(),
            "directory",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert!(root.join("directory/child").is_file());
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            new(),
            "directory",
            SeedAccess::ReadWrite,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();

    assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(root.join("create-new")).unwrap(), b"old");
    assert_eq!(std::fs::read(root.join("absent")).unwrap(), b"new");
    assert_eq!(std::fs::read(root.join("replace")).unwrap(), b"new");
    assert_eq!(permissions_mode(&root.join("replace")) & 0o222, 0);
    assert_eq!(
        failed_over_directory.io_kind(),
        Some(std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(root.join("directory")).unwrap(), b"new");
    assert!(
        std::fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".golem-copy-")),
        "a temporary file must not stay in the root"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_symlink_entries_follow_the_placement() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    std::os::unix::fs::symlink("new-target", sources.path().as_path().join("link")).unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    ["create-new", "replace"].into_iter().for_each(|name| {
        std::os::unix::fs::symlink("old-target", root.join(name)).unwrap();
    });
    std::fs::create_dir(root.join("directory")).unwrap();
    std::fs::write(root.join("directory/child"), b"child").unwrap();
    let link = || host_child_path(&sources, "link");

    let failed = seed_one_entry(
        &filesystem,
        make_seed_entry(
            link(),
            "create-new",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();

    seed_one_entry(
        &filesystem,
        make_seed_entry(
            link(),
            "replace",
            SeedAccess::FromSource,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            link(),
            "absent",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();
    seed_one_entry(
        &filesystem,
        make_seed_entry(
            link(),
            "directory",
            SeedAccess::FromSource,
            SeedPlacement::Replace,
        ),
    )
    .await
    .unwrap();

    assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    [
        ("create-new", "old-target"),
        ("replace", "new-target"),
        ("absent", "new-target"),
        ("directory", "new-target"),
    ]
    .into_iter()
    .for_each(|(name, expected)| {
        assert_eq!(
            std::fs::read_link(root.join(name)).unwrap(),
            PathBuf::from(expected),
            "{name} must point to {expected}"
        );
    });
    assert!(
        std::fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".golem-copy-")),
        "a temporary symlink must not stay in the root"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_reads_a_source_symlink_without_following_it() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"secret").unwrap();
    std::fs::create_dir(sources.path().as_path().join("real")).unwrap();
    std::fs::write(sources.path().as_path().join("real/file"), b"file").unwrap();
    std::os::unix::fs::symlink("real", sources.path().as_path().join("alias")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        sources.path().as_path().join("outside"),
    )
    .unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([
            make_seed_entry(
                host_child_path(&sources, "alias"),
                "alias",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
            make_seed_entry(
                host_child_path(&sources, "outside"),
                "outside",
                SeedAccess::FromSource,
                SeedPlacement::CreateNew,
            ),
        ]),
    )
    .await
    .unwrap();

    let root = filesystem.root();
    assert!(
        std::fs::symlink_metadata(root.join("alias"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(root.join("alias")).unwrap(),
        PathBuf::from("real")
    );
    assert!(
        std::fs::symlink_metadata(root.join("outside"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(root.join("outside")).unwrap(),
        outside.path().join("secret")
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_cannot_write_outside_the_root() {
    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    std::fs::write(sources.path().as_path().join("file"), b"file").unwrap();
    std::os::unix::fs::symlink("file", sources.path().as_path().join("link")).unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("data")).unwrap();

    let errors = futures::future::join_all(
        [
            ("file", "../escaped", SeedPlacement::CreateNew),
            ("file", "data/file", SeedPlacement::CreateNew),
            ("file", "data/file", SeedPlacement::Replace),
            ("file", "data/nested/file", SeedPlacement::CreateNew),
            ("link", "data/link", SeedPlacement::CreateNew),
            ("link", "data/link", SeedPlacement::Replace),
        ]
        .into_iter()
        .map(|(source, target, placement)| {
            let entry = make_seed_entry(
                host_child_path(&sources, source),
                target,
                SeedAccess::FromSource,
                placement,
            );
            let filesystem = &filesystem;
            async move { (target, placement, seed_one_entry(filesystem, entry).await) }
        }),
    )
    .await;

    errors.into_iter().for_each(|(target, placement, result)| {
        assert_eq!(
            result.unwrap_err().io_kind(),
            Some(std::io::ErrorKind::PermissionDenied),
            "seed to {target} with {placement:?} must be refused"
        );
    });
    let tree = host_child_path(&sources, "tree");
    std::fs::create_dir_all(tree.as_path().join("data/nested")).unwrap();
    std::fs::write(tree.as_path().join("data/nested/file"), b"file").unwrap();
    let fail = seed_one_entry(
        &filesystem,
        make_seed_entry(
            tree.clone(),
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert!(
        std::fs::symlink_metadata(root.join("data"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "a failed seed must leave the symlink in place"
    );
    seed_one_entry(
        &filesystem,
        make_seed_entry(tree, "", SeedAccess::FromSource, SeedPlacement::Replace),
    )
    .await
    .unwrap();

    assert_eq!(fail.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert!(root.join("data/nested/file").is_file());
    assert!(
        !std::fs::symlink_metadata(root.join("data"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "replace must put a directory in place of the symlink"
    );
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    assert!(!root.parent().unwrap().join("escaped").exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
async fn seed_directory_entries_merge_and_follow_the_placement() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let sources = create_scratch_directory(parent.path()).await;
    let tree = sources.path().as_path().join("tree");
    std::fs::create_dir_all(tree.join("data")).unwrap();
    std::fs::write(tree.join("data/added"), b"added").unwrap();
    std::fs::write(tree.join("data/same"), b"new").unwrap();
    std::fs::create_dir_all(tree.join("fresh")).unwrap();
    std::fs::write(tree.join("fresh/deep"), b"deep").unwrap();
    std::fs::set_permissions(tree.join("fresh"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::set_permissions(tree.join("data"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::os::unix::fs::symlink("new", tree.join("link")).unwrap();
    std::fs::write(tree.join("was-directory"), b"file").unwrap();
    std::fs::create_dir(tree.join("was-file")).unwrap();
    std::fs::write(tree.join("was-file/inner"), b"inner").unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let root = filesystem.root().to_path_buf();
    ["create-new", "replace"].into_iter().for_each(|placement| {
        let target = root.join(placement);
        std::fs::create_dir_all(target.join("data")).unwrap();
        std::fs::set_permissions(target.join("data"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::write(target.join("data/same"), b"old").unwrap();
        std::fs::write(target.join("data/only-sandbox"), b"sandbox").unwrap();
        std::os::unix::fs::symlink("old", target.join("link")).unwrap();
        std::fs::create_dir(target.join("was-directory")).unwrap();
        std::fs::write(target.join("was-directory/child"), b"child").unwrap();
        std::fs::write(target.join("was-file"), b"file").unwrap();
    });
    std::fs::write(root.join("top-file"), b"top").unwrap();
    let seed = |target: &'static str, placement| {
        let entry = make_seed_entry(
            host_child_path(&sources, "tree"),
            target,
            SeedAccess::FromSource,
            placement,
        );
        let filesystem = &filesystem;
        async move { seed_one_entry(filesystem, entry).await }
    };

    let failed = seed("create-new", SeedPlacement::CreateNew)
        .await
        .unwrap_err();

    seed("replace", SeedPlacement::Replace).await.unwrap();
    seed("absent", SeedPlacement::CreateNew).await.unwrap();
    let failed_top = seed("top-file", SeedPlacement::CreateNew)
        .await
        .unwrap_err();
    assert_eq!(std::fs::read(root.join("top-file")).unwrap(), b"top");
    seed("top-file", SeedPlacement::Replace).await.unwrap();

    assert_eq!(failed.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(
        std::fs::read(root.join("create-new/data/same")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(root.join("create-new/data/added")).unwrap(),
        b"added",
        "a path before the failed path must stay"
    );
    assert!(
        !root.join("create-new/fresh").exists(),
        "a path after the failed path must not be seeded"
    );
    assert_eq!(
        failed_top.io_kind(),
        Some(std::io::ErrorKind::AlreadyExists)
    );

    let read = |path: &str| std::fs::read(root.join(path)).unwrap();

    assert_eq!(read("replace/data/same"), b"new");
    assert_eq!(read("replace/data/added"), b"added");
    assert_eq!(read("replace/data/only-sandbox"), b"sandbox");
    assert_eq!(read("replace/fresh/deep"), b"deep");
    assert_eq!(read("replace/was-file/inner"), b"inner");
    assert_eq!(read("replace/was-directory"), b"file");
    assert_eq!(
        std::fs::read_link(root.join("replace/link")).unwrap(),
        PathBuf::from("new")
    );

    assert_eq!(
        tree_copy::tree_listing(&root.join("absent")),
        tree_copy::tree_listing(&tree)
    );
    assert_eq!(read("top-file/data/same"), b"new");
    assert_eq!(
        permissions_mode(&root.join("replace/data")),
        0o700,
        "a merged directory must keep its permissions"
    );
    assert_eq!(
        permissions_mode(&root.join("replace/fresh")),
        0o750,
        "a made directory must get the permissions of its source"
    );
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
async fn external_seed_keeps_the_pinned_destination_after_directory_rename() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    assert_pinned_seed_after_directory_rename(parent.path(), provisioning).await;
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn apfs_seed_keeps_the_pinned_destination_after_directory_rename() {
    let parent = tempfile::tempdir().unwrap();
    let storage = FilesystemStorageMode::Apfs {
        root: parent.path().join("agents").into_boxed_path(),
    };
    let provisioning =
        SandboxFilesystemProvisioning::new(&storage, RetryConfig::default()).unwrap();
    assert_pinned_seed_after_directory_rename(parent.path(), provisioning).await;
}

#[test]
async fn root_relative_seed_reuses_root_without_descriptor_duplication() {
    let parent = tempfile::tempdir().unwrap();
    let provisioning = directory_sandbox_provisioning(parent.path().to_path_buf());
    let filesystem = <SandboxFilesystem as SandboxFilesystemAdapter>::create_fresh(
        provisioning,
        sandbox_name(),
        None,
    )
    .await
    .unwrap();
    let host_source = create_scratch_directory(parent.path()).await;
    std::fs::write(
        host_source.path().as_path().join("source"),
        b"seeded contents",
    )
    .unwrap();
    let source = || host_child_path(&host_source, "source");
    let root_directory = directory_for(
        &filesystem.root_directory_state(),
        &SandboxPath::at_root("."),
    )
    .unwrap();
    let probe = CapabilityCopyParentProbe::install(root_directory);

    seed_one_entry(
        &filesystem,
        make_seed_entry(
            source(),
            "destination",
            SeedAccess::ReadOnly,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap();

    assert_eq!(
        probe.reused_base(),
        Some(true),
        "root-relative seed duplicated the root capability"
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("destination")).unwrap(),
        b"seeded contents"
    );
    assert!(
        std::fs::metadata(filesystem.root().join("destination"))
            .unwrap()
            .permissions()
            .readonly()
    );

    let existing = seed_one_entry(
        &filesystem,
        make_seed_entry(
            source(),
            "destination",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(
        std::fs::read(filesystem.root().join("destination")).unwrap(),
        b"seeded contents"
    );
    let escaped_path = filesystem.root().parent().unwrap().join("escaped-seed");
    let escaped = seed_one_entry(
        &filesystem,
        make_seed_entry(
            source(),
            "../escaped-seed",
            SeedAccess::ReadWrite,
            SeedPlacement::CreateNew,
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(
        escaped.io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(!escaped_path.exists());
    assert!(std::fs::read_dir(filesystem.root()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".golem-copy-")
    }));

    drop(probe);
    <SandboxFilesystem as SandboxFilesystemAdapter>::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
async fn an_admitted_macos_root_retains_its_unlinked_regular_flusher_after_retirement() {
    let parent = tempfile::tempdir().unwrap();
    let filesystem = directory_sandbox_provisioning(parent.path().to_path_buf())
        .create_fresh(sandbox_name())
        .await
        .unwrap();
    let state = filesystem.root_directory_state();
    let admitted = admit_root(&state).unwrap();
    let metadata = admitted
        .seed_flusher
        .as_deref()
        .unwrap()
        .metadata()
        .unwrap();
    assert!(metadata.is_file());
    assert_eq!(cap_fs_ext::MetadataExt::nlink(&metadata), 0);
    assert!(
        std::fs::read_dir(filesystem.root())
            .unwrap()
            .next()
            .is_none()
    );
    filesystem.root.close();
    assert_eq!(
        admit_root(&state).err().unwrap().kind(),
        std::io::ErrorKind::NotConnected
    );
    tree_copy::sync_seed_batch(
        tree_copy::SeedSyncPolicy::new(tree_copy::SeedSyncPlatform::MacOs, FileTransfer::Bytes),
        Some(&admitted),
        filesystem.root(),
    )
    .unwrap();
    assert!(
        admitted
            .seed_flusher
            .as_deref()
            .unwrap()
            .metadata()
            .unwrap()
            .is_file()
    );
    drop(admitted);
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn apfs_temporary_clone_open_failure_removes_the_native_clone() {
    use std::os::unix::fs::PermissionsExt;

    let source_root = tempfile::tempdir().unwrap();
    let mut source = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(source_root.path().join("source"))
        .unwrap();
    source.write_all(&[b'Z'; 4096]).unwrap();
    source
        .set_permissions(std::fs::Permissions::from_mode(0o644))
        .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let directory = cap_std::fs::Dir::from_std_file(File::open(destination.path()).unwrap());
    let name = temporary_copy_name();

    clone_file_at(&source, &directory, &name).unwrap();
    assert_eq!(directory.read(&name).unwrap().as_slice(), &[b'Z'; 4096]);
    std::fs::set_permissions(
        destination.path().join(&name),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let control_error = directory.open(&name).unwrap_err();
    assert_eq!(control_error.kind(), std::io::ErrorKind::PermissionDenied);

    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(cap_fs_ext::FollowSymlinks::No);
    let error = open_cloned_file(
        || directory.open_with(&name, &options),
        || directory.remove_file(&name),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(error.raw_os_error(), control_error.raw_os_error());
    assert_eq!(directory.entries().unwrap().count(), 0);
    assert_eq!(
        source.metadata().unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(source.metadata().unwrap().len(), 4096);
    assert_eq!(
        std::fs::read(source_root.path().join("source"))
            .unwrap()
            .as_slice(),
        &[b'Z'; 4096],
    );
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::tag(native_apfs)]
fn apfs_temporary_clone_returns_a_clone_failure_without_another_name() {
    let target = tempfile::tempdir().unwrap();
    let path = target.path().to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    // A clone that tried the failure again under another name would never return, so the
    // clone runs on its own thread and the test waits for it with a deadline.
    std::thread::spawn(move || {
        let source = File::open("/System/Library/CoreServices/SystemVersion.plist").unwrap();
        let directory = cap_std::fs::Dir::from_std_file(File::open(&path).unwrap());
        let _ = sender.send(CapabilityTempFile::from_clone(&directory, &source).map(|_| ()));
    });
    let error = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("a failed clone must end the temporary clone")
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
    assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::timeout("60s")]
#[test_r::tag(native_apfs)]
async fn apfs_seed_does_not_scan_an_unrelated_directory() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::Apfs {
                root: root.path().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
    let filesystem = provisioning
        .create_fresh(
            SandboxFilesystemName::new(
                "environment".into(),
                "component".into(),
                "apfs-seed-scope".into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let source_directory = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("seed-source"),
    )
    .await
    .unwrap();
    let source = source_directory
        .path()
        .child(std::ffi::OsStr::new("source"))
        .unwrap();
    std::fs::write(source.as_path(), b"seeded").unwrap();
    let unrelated = filesystem.root().join("unrelated").into_boxed_path();
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o0)).unwrap();
    let result = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([SeedEntry {
            source,
            target: SandboxPath::at_root("new/ancestor/file"),
            access: SeedAccess::ReadWrite,
            placement: SeedPlacement::CreateNew,
        }]),
    )
    .await;
    std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        result.is_ok(),
        "a seed must not scan unrelated directories: {result:?}"
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("new/ancestor/file")).unwrap(),
        b"seeded"
    );
    source_directory.discard().await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
#[test_r::timeout("120s")]
#[test_r::tag(native_apfs)]
async fn apfs_copy_contents_and_seed_share_extents_and_preserve_unflushed_writes() {
    const FILE_BYTES: usize = 64 * 1024 * 1024;
    let root = tempfile::tempdir().unwrap();
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::Apfs {
                root: root.path().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
    let filesystem = provisioning
        .create_fresh(
            SandboxFilesystemName::new(
                "environment".into(),
                "component".into(),
                "apfs-copy".into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let path = filesystem.root().join("file").into_boxed_path();
    let mut source = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    source
        .write_all(&vec![0x5a; FILE_BYTES].into_boxed_slice())
        .unwrap();
    source.sync_all().unwrap();
    source.seek(SeekFrom::Start(0)).unwrap();
    source.write_all(b"unflushed").unwrap();
    source.seek(SeekFrom::End(0)).unwrap();
    source.write_all(b"tail").unwrap();
    std::fs::hard_link(&path, filesystem.root().join("other-name")).unwrap();
    std::fs::write(filesystem.root().join("excluded"), b"excluded").unwrap();
    let capture =
        HostDirectory::create_in(directories.scratch.path(), std::ffi::OsStr::new("copy"))
            .await
            .unwrap();
    let available = |space| match space {
        FilesystemSpace::Observed {
            available_bytes, ..
        } => available_bytes,
        _ => panic!("APFS must report free bytes"),
    };
    let before = available(observe_space_blocking(provisioning.volume()).unwrap());
    let groups = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::new([PathBuf::from("excluded")])),
        capture.path(),
    )
    .await
    .unwrap();
    let after = available(observe_space_blocking(provisioning.volume()).unwrap());
    assert!(
        before.saturating_sub(after) < FILE_BYTES as u64 / 4,
        "a clone must not allocate the file bytes"
    );
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].first.as_ref(), Path::new("file"));
    assert_eq!(
        groups[0].others.as_ref(),
        [Box::from(Path::new("other-name"))]
    );
    assert!(!capture.path().as_path().join("excluded").exists());
    assert!(!capture.path().as_path().join("other-name").exists());
    let copied = std::fs::read(capture.path().as_path().join("file"))
        .unwrap()
        .into_boxed_slice();
    assert_eq!(copied.len(), FILE_BYTES + 4);
    assert_eq!(&copied[..9], b"unflushed");
    assert_eq!(&copied[FILE_BYTES..], b"tail");
    let before_seed = available(observe_space_blocking(provisioning.volume()).unwrap());
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([SeedEntry {
            source: capture.path().clone(),
            target: SandboxPath::at_root("seeded"),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        }]),
    )
    .await
    .unwrap();
    let after_seed = available(observe_space_blocking(provisioning.volume()).unwrap());
    assert!(
        before_seed.saturating_sub(after_seed) < FILE_BYTES as u64 / 4,
        "a seed clone must not allocate the file bytes"
    );
    source.seek(SeekFrom::Start(0)).unwrap();
    source.write_all(b"changed").unwrap();
    assert_eq!(
        std::fs::read(filesystem.root().join("seeded/file"))
            .unwrap()
            .as_slice(),
        copied.as_ref()
    );
    assert_eq!(
        std::fs::read(capture.path().as_path().join("file"))
            .unwrap()
            .as_slice(),
        copied.as_ref()
    );
    capture.discard().await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
fn file_transfer_reflinks_on_copy_on_write_storage_into_the_project_when_there_is_one() {
    let project_id = NonZeroU32::new(7).unwrap();
    let project = QuotaAuthority::Project {
        project_id,
        filesystem_block_bytes: NonZeroU64::new(4096).unwrap(),
    };
    let linux = tree_copy::SeedSyncPlatform::Linux;
    assert_eq!(
        [
            file_transfer(linux, FileCopyMode::Buffered, QuotaAuthority::Unsupported),
            file_transfer(linux, FileCopyMode::Buffered, project),
            file_transfer(linux, FileCopyMode::Reflink, QuotaAuthority::Unsupported),
            file_transfer(linux, FileCopyMode::Reflink, project),
            file_transfer(
                tree_copy::SeedSyncPlatform::MacOs,
                FileCopyMode::Reflink,
                QuotaAuthority::Unsupported,
            ),
        ],
        [
            FileTransfer::Bytes,
            FileTransfer::Bytes,
            FileTransfer::Reflink,
            FileTransfer::ReflinkIntoProject(project_id),
            FileTransfer::Clone,
        ]
    );
}

/// Whether two paths name the same directory. A host directory path on managed storage starts
/// at the descriptor of the volume root (`/proc/self/fd/<n>`), not at the mount path, so the
/// test compares the device and the inode.
#[cfg(target_os = "linux")]
fn is_same_directory(left: &Path, right: &Path) -> bool {
    let left = std::fs::metadata(left).unwrap();
    let right = std::fs::metadata(right).unwrap();
    let identity = |metadata: &std::fs::Metadata| {
        (
            std::os::unix::fs::MetadataExt::dev(metadata),
            std::os::unix::fs::MetadataExt::ino(metadata),
        )
    };
    left.is_dir() && identity(&left) == identity(&right)
}

#[cfg(target_os = "linux")]
fn managed_test_root() -> PathBuf {
    std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
}

#[cfg(target_os = "linux")]
fn managed_test_name(filesystem: &str) -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "native-test-environment".to_string(),
        "native-test-component".to_string(),
        format!("{filesystem}-{}", std::process::id()),
    )
    .unwrap()
}

#[cfg(target_os = "linux")]
fn volume_root(provisioning: &SandboxFilesystemProvisioning) -> Arc<File> {
    Arc::clone(
        provisioning
            .volume()
            .copy_on_write_root()
            .expect("managed test provisioning must have a managed volume"),
    )
}

#[cfg(target_os = "linux")]
fn filesystem_block_bytes(root: &File) -> u64 {
    rustix::fs::fstatfs(root).unwrap().f_bsize as u64
}

/// Reads the allocation of the project of `filesystem` after XFS frees its unlinked inodes.
///
/// XFS keeps an unlinked inode, with its blocks, charged to its project until background
/// inactivation frees it. The reading is the first one whose object count equals the inodes
/// that are reachable from the root. The readings are 25 ms apart. After 5 s the function
/// panics with the last reading and the reachable count.
#[cfg(target_os = "linux")]
async fn settled_allocation(filesystem: &SandboxFilesystem) -> FilesystemAllocation {
    use futures::StreamExt as _;

    let reachable = reachable_inodes(filesystem.root());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let last = futures::stream::unfold(Some(Duration::ZERO), move |delay| async move {
        tokio::time::sleep(delay?).await;
        let reading = filesystem.observe_allocation().await.unwrap();
        let next = (reading.filesystem_objects != reachable
            && tokio::time::Instant::now() < deadline)
            .then_some(Duration::from_millis(25));
        Some((reading, next))
    })
    .fold(None, |_, reading| std::future::ready(Some(reading)))
    .await
    .expect("the first allocation reading always happens");
    assert_eq!(
        last.filesystem_objects, reachable,
        "project allocation did not settle within 5 s: last reading {last:?}, reachable inodes {reachable}"
    );
    last
}

/// Counts the root and every entry below it, without following symlinks.
#[cfg(target_os = "linux")]
fn reachable_inodes(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .fold(1, |count, entry| {
            match entry.file_type().unwrap().is_dir() {
                true => count + reachable_inodes(&entry.path()),
                false => count + 1,
            }
        })
}

#[cfg(target_os = "linux")]
async fn available_bytes(provisioning: &SandboxFilesystemProvisioning) -> u64 {
    rustix::fs::syncfs(&*volume_root(provisioning)).unwrap();
    match crate::storage_volume::observe_space(provisioning.volume())
        .await
        .unwrap()
    {
        FilesystemSpace::Observed {
            available_bytes, ..
        } => available_bytes,
        FilesystemSpace::Unlimited => panic!("managed volume must observe space"),
    }
}

#[cfg(target_os = "linux")]
fn reflink_test_root() -> PathBuf {
    std::env::var_os("GOLEM_REFLINK_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_REFLINK_XFS_TEST_ROOT must name the mounted XFS test root without quotas")
}

#[cfg(target_os = "linux")]
fn reflink_storage(root: PathBuf) -> FilesystemStorageMode {
    FilesystemStorageMode::ReflinkXfs { root: root.into() }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("120s")]
async fn managed_xfs_copy_contents_shares_extents_and_charges_no_agent_quota() {
    use std::os::unix::fs::PermissionsExt;

    const FILE_COUNT: usize = 48;
    const FILE_BYTES: usize = 64 * 1024;

    let root = managed_test_root();
    [".scratch", ".initial-files"].into_iter().for_each(|name| {
        let stale = root.join(name).join("stale-copy");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("garbage"), b"stale").unwrap();
    });
    let inherited_project = NonZeroU32::new(0x7fff_0001).unwrap();
    assign_project(&File::open(&root).unwrap(), inherited_project).unwrap();
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::ManagedXfs {
                xfs_quotad_socket: crate::storage_volume::in_process_xfs_quotad(&root),
                root: root.clone().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
    let ExecutorDirectories {
        scratch: copies,
        initial_files,
    } = directories;
    let copies_root = root.join(".scratch");
    assert!(
        is_same_directory(copies.path().as_path(), &copies_root),
        "the scratch directory must be .scratch under the mount"
    );
    assert!(
        is_same_directory(initial_files.path().as_path(), &root.join(".initial-files")),
        "the initial-files directory must be .initial-files under the mount"
    );
    [copies_root.clone(), root.join(".initial-files")]
        .into_iter()
        .for_each(|host_directory| {
            assert!(
                std::fs::read_dir(&host_directory).unwrap().next().is_none(),
                "{} must hold nothing that an earlier process left",
                host_directory.display()
            );
            let attributes = get_fsxattr(&File::open(&host_directory).unwrap()).unwrap();
            assert_eq!(attributes.fsx_projid, 0);
            assert_eq!(
                attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT,
                0
            );
        });
    let root_attributes = get_fsxattr(&File::open(&root).unwrap()).unwrap();
    assert_eq!(root_attributes.fsx_projid, 0);
    assert_eq!(
        root_attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT,
        0
    );
    let volume_root = volume_root(&provisioning);
    let block_bytes = filesystem_block_bytes(&volume_root);

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-copy"))
        .await
        .unwrap();
    let project_id = filesystem.project_id_for_test();
    let agent_root = filesystem.root().to_path_buf();
    std::fs::create_dir_all(agent_root.join("data/nested")).unwrap();
    std::fs::create_dir(agent_root.join("static")).unwrap();
    (0..FILE_COUNT).for_each(|index| {
        std::fs::write(
            agent_root.join(format!("data/file-{index}")),
            vec![index as u8; FILE_BYTES],
        )
        .unwrap();
    });
    std::fs::write(agent_root.join("data/nested/hidden"), b"excluded directory").unwrap();
    std::fs::write(agent_root.join("static/asset.bin"), b"excluded file").unwrap();
    std::fs::write(agent_root.join("config.toml"), b"[config]").unwrap();
    std::fs::set_permissions(
        agent_root.join("config.toml"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    std::os::unix::fs::symlink("data/file-0", agent_root.join("link")).unwrap();
    std::fs::hard_link(agent_root.join("link"), agent_root.join("link-name")).unwrap();
    std::fs::hard_link(
        agent_root.join("data/file-1"),
        agent_root.join("data/file-1-name"),
    )
    .unwrap();
    rustix::fs::syncfs(&*volume_root).unwrap();

    let unsynced = vec![0xa5; FILE_BYTES];
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(agent_root.join("data/file-0"))
            .unwrap();
        file.write_all(&unsynced).unwrap();
    }
    let available_before = match crate::storage_volume::observe_space(provisioning.volume())
        .await
        .unwrap()
    {
        FilesystemSpace::Observed {
            available_bytes, ..
        } => available_bytes,
        FilesystemSpace::Unlimited => panic!("managed volume must observe space"),
    };
    let allocation_before = filesystem.observe_allocation().await.unwrap();

    let excluded = Arc::new(TreeExclusions::new([
        PathBuf::from("static/asset.bin"),
        PathBuf::from("data/nested"),
    ]));
    let copy = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("copy"))
        .await
        .unwrap();
    let groups = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        excluded,
        copy.path(),
    )
    .await
    .unwrap();
    let copy_root = copy.path().as_path().to_path_buf();

    let available_after = available_bytes(&provisioning).await;
    let copy_bytes = available_before.saturating_sub(available_after);
    let tree_bytes = (FILE_COUNT * FILE_BYTES) as u64;
    println!("COPY_METADATA_BYTES={copy_bytes} tree_bytes={tree_bytes} block_bytes={block_bytes}");
    assert!(
        copy_bytes <= 32 * block_bytes,
        "the copy consumed {copy_bytes} bytes for a {tree_bytes} byte tree"
    );
    let allocation_after = filesystem.observe_allocation().await.unwrap();
    assert_eq!(allocation_after, allocation_before);

    assert_eq!(copy_root.parent(), Some(copies.path().as_path()));
    assert_eq!(
        std::fs::read(copy_root.join("data/file-0")).unwrap(),
        unsynced
    );
    (1..FILE_COUNT).for_each(|index| {
        assert_eq!(
            std::fs::read(copy_root.join(format!("data/file-{index}"))).unwrap(),
            vec![index as u8; FILE_BYTES],
            "data/file-{index} in the copy must have the bytes of the agent file"
        );
    });
    assert_eq!(
        groups.as_ref(),
        [
            LinkGroup {
                first: Path::new("data/file-1").into(),
                others: Box::new([Box::from(Path::new("data/file-1-name"))]),
            },
            LinkGroup {
                first: Path::new("link").into(),
                others: Box::new([Box::from(Path::new("link-name"))]),
            },
        ]
    );
    let mut expected = tree_copy::tree_listing(&agent_root);
    [
        "static/asset.bin",
        "data/nested",
        "data/nested/hidden",
        "data/file-1-name",
        "link-name",
    ]
    .into_iter()
    .for_each(|absent| {
        assert!(
            expected.remove(absent),
            "{absent} must be in the agent tree listing"
        );
    });
    assert_eq!(tree_copy::tree_listing(&copy_root), expected);
    assert_eq!(
        std::fs::metadata(copy_root.join("config.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    assert_eq!(
        std::fs::read_link(copy_root.join("link")).unwrap(),
        PathBuf::from("data/file-0")
    );
    ["data", "data/file-1", "config.toml"]
        .into_iter()
        .for_each(|relative| {
            assert_eq!(
                file_project_id(&File::open(copy_root.join(relative)).unwrap()).unwrap(),
                None,
                "{relative} in the copy must belong to no project"
            );
        });
    assert_eq!(
        file_project_id(&File::open(agent_root.join("data/file-1")).unwrap()).unwrap(),
        Some(project_id)
    );

    copy.discard().await.unwrap();
    assert!(!copy_root.exists());
    copies.discard().await.unwrap();
    assert!(!copies_root.exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert_eq!(
        provisioning
            .project_allocation_for_test(project_id)
            .unwrap(),
        FilesystemAllocation {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("120s")]
async fn managed_xfs_write_after_copy_contents_consumes_one_cow_extent() {
    const FILE_BYTES: usize = 1024 * 1024;

    let root = managed_test_root();
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::ManagedXfs {
                xfs_quotad_socket: crate::storage_volume::in_process_xfs_quotad(&root),
                root: root.clone().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
    let volume_root = volume_root(&provisioning);
    let block_bytes = filesystem_block_bytes(&volume_root);
    let cow_extent_size = cow_extent_size_hint(&volume_root, block_bytes).unwrap();

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-cow"))
        .await
        .unwrap();
    let agent_file = filesystem.root().join("db");
    std::fs::write(&agent_file, vec![0x11; FILE_BYTES]).unwrap();
    let copies = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("native-test-cow-copies"),
    )
    .await
    .unwrap();
    let copy = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("copy"))
        .await
        .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::default()),
        copy.path(),
    )
    .await
    .unwrap();
    let available_before = available_bytes(&provisioning).await;
    let allocation_before = filesystem.observe_allocation().await.unwrap();

    let written = vec![0x22u8; block_bytes as usize];
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&agent_file)
        .unwrap();
    file.write_all(&written).unwrap();
    file.sync_all().unwrap();

    let available_while_open = available_bytes(&provisioning).await;
    let consumed = available_before.saturating_sub(available_while_open);
    let allocation_after = filesystem.observe_allocation().await.unwrap();
    let charged = allocation_after
        .allocated_bytes
        .saturating_sub(allocation_before.allocated_bytes);
    drop(file);
    let available_after_close = available_bytes(&provisioning).await;
    let kept_after_close = available_before.saturating_sub(available_after_close);
    println!(
        "COW_EXTENT_SIZE_BYTES={consumed} charged_to_agent={charged} kept_after_close={kept_after_close} volume_hint={cow_extent_size} block_bytes={block_bytes}"
    );
    assert_eq!(
        consumed, cow_extent_size,
        "a {block_bytes} byte write into a shared extent must consume one copy-on-write extent"
    );
    // The agent project gains at most the new extent. The overwritten block stays allocated
    // because the copy still maps it.
    assert!(
        charged > 0 && charged <= cow_extent_size,
        "the agent project must pay for the copy-on-write extent and nothing more: charged={charged}"
    );
    assert!(
        (block_bytes..=cow_extent_size).contains(&kept_after_close),
        "the changed extent must stay allocated after the file is closed"
    );
    assert_eq!(
        &std::fs::read(&agent_file).unwrap()[..block_bytes as usize],
        &written[..]
    );
    assert_eq!(
        std::fs::read(copy.path().as_path().join("db")).unwrap(),
        vec![0x11; FILE_BYTES]
    );

    copy.discard().await.unwrap();
    copies.discard().await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("120s")]
async fn managed_xfs_seed_charges_the_project_and_follows_the_placement() {
    let root = managed_test_root();
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &FilesystemStorageMode::ManagedXfs {
                xfs_quotad_socket: crate::storage_volume::in_process_xfs_quotad(&root),
                root: root.clone().into(),
            },
            RetryConfig::default(),
        )
        .await
        .unwrap();
    let sources = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("native-test-seed-sources"),
    )
    .await
    .unwrap();
    let sources_root = sources.path().as_path().to_path_buf();
    assert_eq!(
        file_project_id(&File::open(&sources_root).unwrap()).unwrap(),
        None
    );
    let tree = sources.path().child(std::ffi::OsStr::new("tree")).unwrap();
    std::fs::create_dir_all(tree.as_path().join("data")).unwrap();
    std::fs::write(tree.as_path().join("data/large"), vec![0x31; 256 * 1024]).unwrap();
    std::fs::write(tree.as_path().join("data/small"), vec![0x32; 64 * 1024]).unwrap();
    std::os::unix::fs::symlink("data/small", tree.as_path().join("link")).unwrap();
    let replacement = sources
        .path()
        .child(std::ffi::OsStr::new("replacement"))
        .unwrap();
    std::fs::write(replacement.as_path(), vec![0x33; 128 * 1024]).unwrap();
    let entry = |source: &HostPath, target: &str, access, placement| SeedEntry {
        source: source.clone(),
        target: SandboxPath::at_root(target),
        access,
        placement,
    };

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-seed"))
        .await
        .unwrap();
    let project_id = filesystem.project_id_for_test();
    let allocation_before = settled_allocation(&filesystem).await;

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        )]),
    )
    .await
    .unwrap();

    let allocation_after = settled_allocation(&filesystem).await;
    assert!(
        allocation_after.allocated_bytes >= allocation_before.allocated_bytes + 320 * 1024,
        "seed did not charge the agent project: before={allocation_before:?}, after={allocation_after:?}"
    );
    assert_eq!(
        allocation_after.filesystem_objects,
        allocation_before.filesystem_objects + 4
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("data/large")).unwrap(),
        vec![0x31; 256 * 1024]
    );
    assert_eq!(
        std::fs::read_link(filesystem.root().join("link")).unwrap(),
        PathBuf::from("data/small")
    );
    ["data", "data/large", "data/small"]
        .into_iter()
        .for_each(|relative| {
            assert_eq!(
                file_project_id(&File::open(filesystem.root().join(relative)).unwrap()).unwrap(),
                Some(project_id),
                "{relative} must belong to the agent project"
            );
        });
    assert!(tree.as_path().join("data/large").is_file());

    let existing = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        )]),
    )
    .await
    .unwrap_err();
    assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(settled_allocation(&filesystem).await, allocation_after);

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &replacement,
            "data/small",
            SeedAccess::ReadOnly,
            SeedPlacement::Replace,
        )]),
    )
    .await
    .unwrap();
    let allocation_replaced = settled_allocation(&filesystem).await;
    assert_eq!(
        std::fs::read(filesystem.root().join("data/small")).unwrap(),
        vec![0x33; 128 * 1024]
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("data/large")).unwrap(),
        vec![0x31; 256 * 1024]
    );
    assert!(
        std::fs::metadata(filesystem.root().join("data/small"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(
        file_project_id(&File::open(filesystem.root().join("data/small")).unwrap()).unwrap(),
        Some(project_id)
    );
    assert!(
        allocation_replaced.allocated_bytes >= allocation_after.allocated_bytes + 64 * 1024,
        "a replacement with more bytes must charge the agent project: before={allocation_after:?}, after={allocation_replaced:?}"
    );
    assert_eq!(
        allocation_replaced.filesystem_objects,
        allocation_after.filesystem_objects
    );

    std::fs::write(filesystem.root().join("data/agent"), vec![0x34; 32 * 1024]).unwrap();
    let allocation_before_replace = settled_allocation(&filesystem).await;
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::Replace,
        )]),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(filesystem.root().join("data/small")).unwrap(),
        vec![0x32; 64 * 1024],
        "replace must put the source file below a merged directory"
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("data/agent")).unwrap(),
        vec![0x34; 32 * 1024],
        "a merge must leave a path that only the sandbox has"
    );
    assert_eq!(
        file_project_id(&File::open(filesystem.root().join("data/small")).unwrap()).unwrap(),
        Some(project_id)
    );
    let allocation_restored = settled_allocation(&filesystem).await;
    assert!(
        allocation_restored.allocated_bytes < allocation_before_replace.allocated_bytes,
        "a replacement with fewer bytes must release project bytes: before={allocation_before_replace:?}, after={allocation_restored:?}"
    );
    assert_eq!(
        allocation_restored.filesystem_objects,
        allocation_before_replace.filesystem_objects
    );

    let limited = provisioning
        .create_fresh(managed_test_name("native-test-seed-limited"))
        .await
        .unwrap();
    let limited_project_id = limited.project_id_for_test();
    limited
        .install_limits(FilesystemLimits {
            allocated_bytes: 128 * 1024,
            filesystem_objects: 64,
        })
        .await
        .unwrap();
    let exhausted = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &limited,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        )]),
    )
    .await
    .unwrap_err();
    assert!(
        exhausted.is_storage_exhaustion(),
        "seed into a small quota must fail with a quota error: {exhausted}"
    );

    sources.discard().await.unwrap();
    assert!(!sources_root.exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
    SandboxFilesystem::delete_and_verify(limited).await.unwrap();
    [project_id, limited_project_id]
        .into_iter()
        .for_each(|project| {
            assert_eq!(
                provisioning.project_allocation_for_test(project).unwrap(),
                FilesystemAllocation {
                    allocated_bytes: 0,
                    filesystem_objects: 0,
                },
                "project {project} must have no allocation after cleanup"
            );
        });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("120s")]
async fn reflink_xfs_copy_contents_and_seed_share_extents() {
    const FILE_COUNT: usize = 48;
    const FILE_BYTES: usize = 64 * 1024;

    let root = reflink_test_root();
    let (provisioning, directories) =
        crate::services::agent_filesystem::provision_executor_directories(
            &reflink_storage(root.clone()),
            RetryConfig::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        provisioning.volume().agent_accounting(),
        AgentAccounting::Unaccounted
    );
    assert!(provisioning.volume().copies_on_write());
    [root.join(".scratch"), root.join(".initial-files")]
        .into_iter()
        .for_each(|host_directory| {
            let attributes = get_fsxattr(&File::open(&host_directory).unwrap()).unwrap();
            assert!(
                !has_project_identity(&attributes),
                "{} must have no project identity",
                host_directory.display()
            );
        });
    let volume_root = volume_root(&provisioning);
    let block_bytes = filesystem_block_bytes(&volume_root);

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-reflink-copy"))
        .await
        .unwrap();
    let agent_root = filesystem.root().to_path_buf();
    std::fs::create_dir_all(agent_root.join("data")).unwrap();
    (0..FILE_COUNT).for_each(|index| {
        std::fs::write(
            agent_root.join(format!("data/file-{index}")),
            vec![index as u8; FILE_BYTES],
        )
        .unwrap();
    });
    std::fs::hard_link(
        agent_root.join("data/file-1"),
        agent_root.join("data/file-1-name"),
    )
    .unwrap();
    rustix::fs::syncfs(&*volume_root).unwrap();
    let tree_bytes = (FILE_COUNT * FILE_BYTES) as u64;

    let available_before_copy = available_bytes(&provisioning).await;
    let copy = HostDirectory::create_in(directories.scratch.path(), std::ffi::OsStr::new("copy"))
        .await
        .unwrap();
    let groups = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::new(std::iter::empty::<PathBuf>())),
        copy.path(),
    )
    .await
    .unwrap();
    let copy_bytes = available_before_copy.saturating_sub(available_bytes(&provisioning).await);
    println!("REFLINK_COPY_BYTES={copy_bytes} tree_bytes={tree_bytes} block_bytes={block_bytes}");
    assert!(
        copy_bytes <= 32 * block_bytes,
        "the copy consumed {copy_bytes} bytes for a {tree_bytes} byte tree"
    );
    assert_eq!(groups.len(), 1);
    (0..FILE_COUNT).for_each(|index| {
        assert_eq!(
            std::fs::read(copy.path().as_path().join(format!("data/file-{index}"))).unwrap(),
            vec![index as u8; FILE_BYTES],
            "data/file-{index} in the copy must have the bytes of the agent file"
        );
    });

    let source = copy.path().child(std::ffi::OsStr::new("data")).unwrap();
    let available_before_seed = available_bytes(&provisioning).await;
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([SeedEntry {
            source,
            target: SandboxPath::at_root("seeded"),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        }]),
    )
    .await
    .unwrap();
    let seed_bytes = available_before_seed.saturating_sub(available_bytes(&provisioning).await);
    println!("REFLINK_SEED_BYTES={seed_bytes} tree_bytes={tree_bytes} block_bytes={block_bytes}");
    assert!(
        seed_bytes <= 32 * block_bytes,
        "the seed consumed {seed_bytes} bytes for a {tree_bytes} byte tree"
    );
    (0..FILE_COUNT).for_each(|index| {
        assert_eq!(
            std::fs::read(agent_root.join(format!("seeded/file-{index}"))).unwrap(),
            vec![index as u8; FILE_BYTES],
            "seeded/file-{index} must have the bytes of the copy"
        );
    });

    copy.discard().await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}
