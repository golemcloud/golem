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
use test_r::test;

#[test]
fn volume_errors_own_their_context_and_keep_cleanup_explicit() {
    let error = StorageVolumeError::io(
        "open volume root",
        Some(Path::new("/volume")),
        std::io::ErrorKind::PermissionDenied.into(),
    );
    assert_eq!(error.context().operation(), "open volume root");
    assert_eq!(error.context().path(), Some(Path::new("/volume")));
    assert_eq!(
        error.context().io_kind(),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    assert!(!error.cleanup_failed());
    assert!(
        error
            .to_string()
            .starts_with("failed to open volume root at /volume:")
    );
    assert!(error.during_cleanup().cleanup_failed());
}

#[test]
fn task_failures_preserve_the_task_kind_instead_of_an_io_wrapper() {
    let error =
        StorageVolumeError::task_failure("observe volume", None, BlockingTaskError::Panicked);
    assert!(error.context().io_kind().is_none());
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .is::<BlockingTaskError>()
    );
    assert!(matches!(
        error.context().task_error(),
        Some(BlockingTaskError::Panicked)
    ));
}

#[test]
fn probe_completion_keeps_primary_and_cleanup_failures_distinct() {
    use std::io::ErrorKind;
    for probe_failed in [false, true] {
        for cleanup_failed in [false, true] {
            let probed = if probe_failed {
                Err(StorageVolumeError::io(
                    "probe volume",
                    None,
                    ErrorKind::Unsupported.into(),
                ))
            } else {
                Ok(())
            };
            let removed = if cleanup_failed {
                Err(StorageVolumeError::io(
                    "remove volume probe",
                    None,
                    ErrorKind::PermissionDenied.into(),
                ))
            } else {
                Ok(())
            };
            let result = StorageVolumeError::after_cleanup(probed, removed);
            if !probe_failed && !cleanup_failed {
                assert!(result.is_ok());
                continue;
            }
            let mut error = result.unwrap_err();
            assert_eq!(error.cleanup_failed(), cleanup_failed);
            let selected_kind = if cleanup_failed {
                ErrorKind::PermissionDenied
            } else {
                ErrorKind::Unsupported
            };
            assert_eq!(error.context().io_kind(), Some(selected_kind));
            if probe_failed && cleanup_failed {
                assert!(error.to_string().contains("failed to probe volume"));
                assert!(
                    error
                        .to_string()
                        .contains("cleanup also failed: failed to remove volume probe")
                );
                let primary = error.take_original_error().unwrap();
                assert!(!primary.cleanup_failed());
                assert_eq!(primary.context().io_kind(), Some(ErrorKind::Unsupported));
            } else {
                assert!(error.take_original_error().is_none());
            }
        }
    }
}
