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

use super::{ResolvedPathEntry, SandboxPath};
use crate::filesystem::sandbox::SandboxFollow;
use std::sync::Arc;
use test_r::test;

#[cfg(target_os = "linux")]
#[test]
fn deferred_followed_permissions_preserve_raw_terminal_errno() {
    let mut target =
        crate::filesystem::sandbox::scripted_adapter::default_scripted_path_resolution(
            SandboxPath::at_root("link"),
        )
        .unwrap();
    let source = std::io::Error::from_raw_os_error(libc::EIO);
    target.entry = ResolvedPathEntry::Symlink {
        referent_read_only: Err(Arc::new(source)),
    };

    assert!(target.is_read_only_file(SandboxFollow::No).is_ok());
    let Err(error) = target.is_read_only_file(SandboxFollow::Yes) else {
        panic!("followed permissions unexpectedly discarded their deferred failure")
    };
    assert_eq!(
        error.io_error().and_then(std::io::Error::raw_os_error),
        Some(libc::EIO)
    );
    assert!(error.is_terminal_failure());
}

#[test]
fn deferred_followed_permissions_preserve_errors_without_an_errno() {
    let mut target =
        crate::filesystem::sandbox::scripted_adapter::default_scripted_path_resolution(
            SandboxPath::at_root("link"),
        )
        .unwrap();
    target.entry = ResolvedPathEntry::Symlink {
        referent_read_only: Err(Arc::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cannot inspect referent",
        ))),
    };

    assert!(!target.is_read_only_file(SandboxFollow::No).unwrap());
    assert!(target.target_directory_id().is_none());
    for resolved in [target.clone(), target] {
        let error = resolved.is_read_only_file(SandboxFollow::Yes).unwrap_err();
        assert_eq!(error.io_kind(), Some(std::io::ErrorKind::PermissionDenied));
        assert_eq!(
            error.io_error().unwrap().to_string(),
            "cannot inspect referent"
        );
        assert!(error.is_terminal_failure());
    }
}
