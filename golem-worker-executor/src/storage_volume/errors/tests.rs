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
fn volume_errors_keep_the_original_io_cause_and_cleanup_status() {
    let error = StorageVolumeError::cleanup_io(
        "remove volume probe",
        Path::new("/volume/probe"),
        std::io::Error::from_raw_os_error(5),
    );
    assert_eq!(error.io_error().unwrap().raw_os_error(), Some(5));
    assert_eq!(
        error.io_kind(),
        Some(std::io::Error::from_raw_os_error(5).kind())
    );
    assert!(error.cleanup_failed());
    let (operation, path, source, cleanup_failed, task_failed) = error.into_parts();
    assert_eq!(operation, "remove volume probe");
    assert_eq!(path, Path::new("/volume/probe"));
    assert_eq!(source.unwrap().raw_os_error(), Some(5));
    assert!(cleanup_failed);
    assert!(!task_failed);
}

#[test]
fn verification_errors_have_no_fabricated_io_cause() {
    let error = StorageVolumeError::verification("validate volume", Path::new("/volume"));
    assert!(error.io_error().is_none());
    assert!(!error.cleanup_failed());
    assert_eq!(
        error.to_string(),
        "failed to validate volume filesystem /volume"
    );
}
