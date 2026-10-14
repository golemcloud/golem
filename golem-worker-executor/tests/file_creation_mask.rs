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

//! The fallback of the file mode creation mask of the executor, in a process of its own: the test
//! changes the mask of its process, which every thread of the process shares.

test_r::enable!();

#[cfg(unix)]
#[allow(dead_code)]
#[path = "../src/services/agent_filesystem/file_creation_mask.rs"]
mod file_creation_mask;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use test_r::test;

/// Creates the file at `path` with the requested mode 0666, and gives the mode that it got.
#[cfg(unix)]
fn created_mode(path: &Path) -> u32 {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o666)
        .open(path)
        .unwrap();
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The process inherits the mask 077. The fallback sets its probe mask, another thread creates a
/// file before the fallback sets the mask again, and a third file comes after. No file gets a
/// group or other permission, and the mask of the process stays 077.
#[cfg(unix)]
#[test]
fn the_fallback_never_gives_a_group_or_other_permission_that_the_inherited_mask_takes_away() {
    let directory = tempfile::tempdir().unwrap();
    // SAFETY: `umask` only replaces the mask of the process. It cannot fail.
    unsafe {
        libc::umask(0o077);
    }
    let before = created_mode(&directory.path().join("before"));

    let mut during = None;
    file_creation_mask::keep_owner_write_permission_with(|| {
        let mask = file_creation_mask::probed_file_creation_mask();
        let path = directory.path().join("during");
        during = Some(
            std::thread::spawn(move || created_mode(&path))
                .join()
                .unwrap(),
        );
        mask
    });
    let after = created_mode(&directory.path().join("after"));
    // SAFETY: `umask` only replaces the mask of the process. It cannot fail.
    let mask = unsafe { libc::umask(0o077) };

    assert_eq!(
        (before, during, after, mask),
        (0o600, Some(0o600), 0o600, 0o077)
    );
}
