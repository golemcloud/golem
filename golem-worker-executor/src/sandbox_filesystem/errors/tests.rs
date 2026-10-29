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
use crate::blocking::BlockingTask;
use crate::operation_failure::OperationContext;
use crate::sandbox_filesystem::errors::FilesystemStorageError;
use crate::storage_volume::StorageVolumeError;
use std::path::Path;
use test_r::test;

#[test]
fn operation_errors_have_no_fabricated_path_and_keep_both_cleanup_diagnostics() {
    let error = FilesystemStorageError::from(OperationContext::verification(
        "create executor storage",
        None,
    ));
    assert_eq!(error.to_string(), "failed to create executor storage");
    assert!(format!("{error:?}").contains("path: None"));
    let cleanup = FilesystemStorageError::cleanup_io(
        "remove scratch",
        Some(Path::new("/scratch")),
        std::io::ErrorKind::ReadOnlyFilesystem.into(),
    );
    let error = error.with_cleanup_failure(cleanup);
    assert!(error.to_string().contains("create executor storage"));
    assert!(
        error
            .to_string()
            .contains("cleanup also failed: failed to remove scratch at /scratch")
    );
    assert!(error.cleanup_failed());
    assert!(error.is_terminal_failure());
    assert_eq!(
        error.io_kind(),
        Some(std::io::ErrorKind::ReadOnlyFilesystem)
    );
}

#[test]
fn volume_error_conversion_preserves_io_and_cleanup_facts() {
    let error = StorageVolumeError::cleanup_io(
        "probe volume",
        Some(Path::new("/volume")),
        std::io::ErrorKind::ReadOnlyFilesystem.into(),
    );
    let message = error.to_string();
    let error = FilesystemStorageError::from(error);
    assert_eq!(error.to_string(), message);
    assert_eq!(
        error.io_kind(),
        Some(std::io::ErrorKind::ReadOnlyFilesystem)
    );
    assert!(error.cleanup_failed());
    assert!(error.is_terminal_failure());
    assert!(!error.is_sandbox_escape());
}

#[test]
fn shared_storage_failures_do_not_become_sandbox_escape_refusals() {
    let source = std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "a path led outside of the filesystem",
    );
    let error = FilesystemStorageError::from(StorageVolumeError::from(OperationContext::io(
        "open volume root",
        None,
        source,
    )));
    assert!(!error.is_sandbox_escape());
    assert!(error.is_terminal_failure());
}

#[test]
async fn volume_task_failure_remains_terminal_after_conversion() {
    let failure = BlockingTask::Spawn
        .run(|| panic!("volume task"))
        .await
        .unwrap_err();
    let error = StorageVolumeError::from(OperationContext::task_failure(
        "observe volume",
        Some(Path::new("/volume")),
        failure,
    ));
    let error = FilesystemStorageError::from(error);
    assert!(error.is_terminal_failure());
    assert!(matches!(
        error.task_error(),
        Some(BlockingTaskError::Panicked)
    ));
    assert!(error.io_kind().is_none());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn unix_storage_errors_are_terminal_only_when_the_storage_is_unavailable() {
    [libc::EIO, libc::ESTALE, libc::ENODEV]
        .into_iter()
        .for_each(|errno| {
            assert!(is_terminal_storage_errno(
                &std::io::Error::from_raw_os_error(errno)
            ));
        });
    [libc::ENOSPC, libc::EAGAIN, libc::EINTR, libc::EXDEV]
        .into_iter()
        .for_each(|errno| {
            assert!(!is_terminal_storage_errno(
                &std::io::Error::from_raw_os_error(errno)
            ));
        });
}

#[test]
fn unsupported_allocation_classification_is_typed() {
    let unsupported = FilesystemStorageError::allocation_unsupported(None);
    let same_message = FilesystemStorageError::from(OperationContext::verification(
        "observe allocation without quota authority",
        None,
    ));

    assert!(unsupported.allocation_is_unsupported());
    assert!(!same_message.allocation_is_unsupported());
    assert_eq!(unsupported.to_string(), same_message.to_string());
}

#[test]
fn native_task_panic_is_caught_and_classified_as_terminal() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let execution_error = runtime
        .block_on(BlockingTask::InPlace.run(|| -> () { panic!("scripted native panic") }))
        .unwrap_err();
    let error = FilesystemStorageError::from(OperationContext::task_failure(
        "run panicking native operation",
        None,
        execution_error,
    ));

    assert!(error.is_terminal_failure());
    assert!(error.io_kind().is_none());
    assert!(matches!(
        error.task_error(),
        Some(BlockingTaskError::Panicked)
    ));
}

#[test]
fn storage_error_is_one_pointer_wide() {
    assert_eq!(
        std::mem::size_of::<FilesystemStorageError>(),
        std::mem::size_of::<usize>()
    );
}

#[test]
fn storage_error_debug_output_names_each_fact() {
    let error = FilesystemStorageError::cleanup_verification(
        "remove directory",
        Some(Path::new("/root/a")),
    );

    let debug = format!("{error:?}");
    for fact in [
        "remove directory",
        "/root/a",
        "Verification",
        "cleanup_failed: true",
        "kind: General",
    ] {
        assert!(debug.contains(fact), "{debug}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn namespace_operation_errors_preserve_raw_terminal_errno() {
    for (operation, errno) in [
        ("hard-link sandbox filesystem path", libc::EIO),
        ("rename sandbox filesystem path", libc::ESTALE),
        ("rename sandbox filesystem path", libc::ENODEV),
    ] {
        let error =
            FilesystemStorageError::io(operation, None, std::io::Error::from_raw_os_error(errno));
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(errno)
        );
        assert!(error.is_terminal_failure());
    }

    let cross_device = FilesystemStorageError::io(
        "hard-link sandbox filesystem path",
        None,
        std::io::Error::from_raw_os_error(libc::EXDEV),
    );
    assert_eq!(
        cross_device
            .io_error()
            .and_then(std::io::Error::raw_os_error),
        Some(libc::EXDEV)
    );
}
