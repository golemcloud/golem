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

use super::*;
#[cfg(unix)]
use cap_std::fs::PermissionsExt as _;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::time::{Duration, UNIX_EPOCH};
use test_r::test;

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
