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

use std::fs::{Metadata, Permissions};
use std::path::Path;

pub(super) fn writable(mut permissions: Permissions) -> Permissions {
    permissions.set_readonly(false);
    permissions
}

pub(in crate::filesystem_snapshot) fn mode_of(metadata: &Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}

pub(super) fn set_mode(path: &Path, mode: u32) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    std::fs::set_permissions(path, permissions).unwrap();
}

pub(super) fn make_symlink(_target: &str, _link: &Path) {
    unreachable!("the fixture has symlinks only on unix");
}
