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
use test_r::test;

#[test]
fn operation_errors_do_not_report_an_invented_path() {
    let error = FilesystemOperationError::verification("validate pressure configuration", None);
    assert_eq!(
        error.to_string(),
        "failed to validate pressure configuration filesystem"
    );
    assert!(error.path.is_none());
}

#[test]
fn errors_keep_the_original_io_cause_and_cleanup_status() {
    let error = FilesystemOperationError::cleanup_io(
        "remove volume probe",
        Some(Path::new("/volume/probe")),
        std::io::Error::from_raw_os_error(5),
    );
    assert_eq!(error.io_error().unwrap().raw_os_error(), Some(5));
    assert_eq!(
        error.io_kind(),
        Some(std::io::Error::from_raw_os_error(5).kind())
    );
    assert_eq!(error.operation, "remove volume probe");
    assert_eq!(error.path.as_deref(), Some(Path::new("/volume/probe")));
    assert!(error.cleanup_failed());
    assert!(!error.task_failed());
}

#[test]
fn verification_errors_have_no_fabricated_io_cause() {
    let error =
        FilesystemOperationError::verification("validate volume", Some(Path::new("/volume")));
    assert!(error.io_error().is_none());
    assert!(!error.cleanup_failed());
    assert_eq!(
        error.to_string(),
        "failed to validate volume filesystem /volume"
    );
}

#[test]
fn removal_errors_keep_verification_and_io_causes_distinct() {
    let error = FilesystemOperationError::removal(
        "remove directory",
        Path::new("/root"),
        RemoveFailure::StillPresent,
    );
    assert!(error.cleanup_failed());
    assert!(error.io_error().is_none());
    let error = FilesystemOperationError::removal(
        "remove probe",
        Path::new("/root/probe"),
        RemoveFailure::Io(std::io::ErrorKind::ReadOnlyFilesystem.into()),
    );
    assert!(error.cleanup_failed());
    assert_eq!(
        error.io_kind(),
        Some(std::io::ErrorKind::ReadOnlyFilesystem)
    );
}

#[test]
async fn task_failures_keep_their_typed_classification() {
    let source = BlockingTask::Spawn
        .run(|| panic!("filesystem task"))
        .await
        .unwrap_err();
    let error = FilesystemOperationError::task_failure("observe capacity", None, source);
    assert!(error.task_failed());
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::Other));
    assert!(!error.cleanup_failed());
}
