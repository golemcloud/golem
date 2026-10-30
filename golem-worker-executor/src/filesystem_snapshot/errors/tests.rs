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
use crate::filesystem_snapshot::{SnapshotInfo, SnapshotName};
use pretty_assertions::assert_eq;
use std::io::ErrorKind;
use test_r::test;

fn name(text: &str) -> SnapshotName {
    SnapshotName::new(text).unwrap()
}

#[test]
fn a_name_and_the_errors_say_what_they_hold() {
    let invalid = SnapshotName::new("a/b").unwrap_err();
    let failed = SaveError::Failed(Failed::new(anyhow::anyhow!("the bucket is gone")));
    let source = SaveError::Source(std::io::Error::new(ErrorKind::NotFound, "no tree"));

    assert_eq!(
        (
            name("p-1").to_string(),
            invalid.to_string(),
            failed.to_string(),
            std::error::Error::source(&failed).map(ToString::to_string),
            std::error::Error::source(&source).map(ToString::to_string),
            std::error::Error::source(&RestoreFailure::NotFound).is_none(),
            CallError::Stopped(Withdrawal::Deadline).to_string(),
        ),
        (
            "p-1".to_string(),
            "the filesystem snapshot name \"a/b\" does not have 1 to 64 ASCII letters, digits, `-` or `_`".to_string(),
            "the filesystem snapshot store could not finish the call: the bucket is gone".to_string(),
            Some("the filesystem snapshot store could not finish the call: the bucket is gone".to_string()),
            Some("no tree".to_string()),
            true,
            "the deadline of the filesystem snapshot call passed".to_string(),
        )
    );
}

/// Each error of a store method fits within 16 bytes.
#[test]
fn each_error_of_the_store_has_at_most_16_bytes() {
    assert_eq!(
        [
            std::mem::size_of::<Failed>(),
            std::mem::size_of::<CallError>(),
            std::mem::size_of::<SaveError>(),
            std::mem::size_of::<RestoreFailure>(),
            std::mem::size_of::<ReadError>(),
            std::mem::size_of::<Result<SnapshotInfo, SaveError>>(),
        ],
        [8, 16, 16, 16, 16, 32]
    );
}
