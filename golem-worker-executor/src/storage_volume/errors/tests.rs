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
    assert_eq!(error.operation(), "open volume root");
    assert_eq!(error.path(), Some(Path::new("/volume")));
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::PermissionDenied));
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
    assert!(error.io_kind().is_none());
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .is::<BlockingTaskError>()
    );
    assert!(matches!(
        error.into_failure(),
        OperationFailure::Task(BlockingTaskError::Panicked)
    ));
}
