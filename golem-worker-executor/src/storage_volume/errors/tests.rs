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
use crate::blocking::BlockingTaskError;
use crate::operation_failure::OperationContext;
use test_r::test;

#[test]
fn consuming_conversion_preserves_the_chain_flag_and_selected_cause() {
    use crate::sandbox_filesystem::FilesystemStorageError;
    let primary = StorageVolumeError::from(OperationContext::io(
        "probe volume",
        None,
        std::io::ErrorKind::Unsupported.into(),
    ));
    let cleanup = StorageVolumeError::from(OperationContext::io(
        "remove probe",
        None,
        std::io::ErrorKind::PermissionDenied.into(),
    ));
    let volume_error = StorageVolumeError::after_cleanup(Err(primary), Err(cleanup)).unwrap_err();
    let message = volume_error.to_string();
    let sandbox_error = FilesystemStorageError::from(volume_error);
    assert_eq!(sandbox_error.to_string(), message);
    assert!(sandbox_error.cleanup_failed());
    assert_eq!(
        sandbox_error.io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(sandbox_error.is_terminal_failure());
    assert!(!sandbox_error.is_sandbox_escape());
}

#[test]
fn volume_errors_own_their_context_and_keep_cleanup_explicit() {
    let error = StorageVolumeError::from(OperationContext::io(
        "open volume root",
        Some(Path::new("/volume")),
        std::io::ErrorKind::PermissionDenied.into(),
    ));
    assert_eq!(
        error.as_ref().context().io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(!error.as_ref().cleanup_failed());
    assert!(
        error
            .to_string()
            .starts_with("failed to open volume root at /volume:")
    );
    assert!(error.during_cleanup().as_ref().cleanup_failed());
}

#[test]
fn task_failures_preserve_the_task_kind_instead_of_an_io_wrapper() {
    let error = StorageVolumeError::from(OperationContext::task_failure(
        "observe volume",
        None,
        BlockingTaskError::Panicked,
    ));
    assert!(error.as_ref().context().io_kind().is_none());
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .is::<BlockingTaskError>()
    );
    assert!(matches!(
        error.as_ref().context().task_error(),
        Some(BlockingTaskError::Panicked)
    ));
}

#[test]
fn probe_completion_keeps_primary_and_cleanup_failures_distinct() {
    use std::io::ErrorKind;
    for probe_failed in [false, true] {
        for cleanup_failed in [false, true] {
            let probed = if probe_failed {
                Err(StorageVolumeError::from(OperationContext::io(
                    "probe volume",
                    None,
                    ErrorKind::Unsupported.into(),
                )))
            } else {
                Ok(())
            };
            let removed = if cleanup_failed {
                Err(StorageVolumeError::from(OperationContext::io(
                    "remove volume probe",
                    None,
                    ErrorKind::PermissionDenied.into(),
                )))
            } else {
                Ok(())
            };
            let result = StorageVolumeError::after_cleanup(probed, removed);
            if !probe_failed && !cleanup_failed {
                assert!(result.is_ok());
                continue;
            }
            let error = result.unwrap_err();
            assert_eq!(error.as_ref().cleanup_failed(), cleanup_failed);
            let selected_kind = if cleanup_failed {
                ErrorKind::PermissionDenied
            } else {
                ErrorKind::Unsupported
            };
            assert_eq!(error.as_ref().context().io_kind(), Some(selected_kind));
            if probe_failed && cleanup_failed {
                assert!(error.to_string().contains("failed to probe volume"));
                assert!(
                    error
                        .to_string()
                        .contains("cleanup also failed: failed to remove volume probe")
                );
            } else {
                assert!(!error.to_string().contains("cleanup also failed"));
            }
        }
    }
}
