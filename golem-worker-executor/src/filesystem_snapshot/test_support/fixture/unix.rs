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
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn writable(permissions: Permissions) -> Permissions {
    Permissions::from_mode(permissions.mode() | 0o700)
}

pub(in crate::filesystem_snapshot) fn mode_of(metadata: &Metadata) -> u32 {
    metadata.permissions().mode() & 0o7777
}

pub(super) fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
}

pub(super) fn make_symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}
