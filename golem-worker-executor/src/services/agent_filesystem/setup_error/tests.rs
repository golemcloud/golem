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
fn setup_preserves_configuration_and_storage_failures_as_distinct_causes() {
    let error = FilesystemSetupError::from(FilesystemPressureConfigError::Bytes {
        target: 20,
        capacity: 10,
    });
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<FilesystemPressureConfigError>());
    assert!(!source.is::<FilesystemStorageError>());
    assert_eq!(
        error.to_string(),
        "filesystem pressure byte target 20 exceeds volume capacity 10"
    );

    let source = FilesystemStorageError::io(
        "observe volume",
        None,
        std::io::ErrorKind::PermissionDenied.into(),
    );
    let message = source.to_string();
    let error = FilesystemSetupError::from(source);
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .is::<FilesystemStorageError>()
    );
    assert_eq!(error.to_string(), message);
}
