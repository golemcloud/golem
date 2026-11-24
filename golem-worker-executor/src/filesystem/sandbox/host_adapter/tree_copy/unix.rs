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

use super::{File, Path, PathBuf, SeedAccess};
use cap_std::fs::PermissionsExt as _;
use std::os::unix::fs::PermissionsExt as _;

/// Gives `permissions` the write permission that `access` sets.
pub(in crate::filesystem::sandbox) fn seeded_permissions(
    mut permissions: std::fs::Permissions,
    access: SeedAccess,
) -> std::fs::Permissions {
    let mode = permissions.mode();
    match access {
        SeedAccess::FromSource => {}
        SeedAccess::ReadOnly => permissions.set_mode(mode & !0o222),
        SeedAccess::ReadWrite => permissions.set_mode(mode | 0o200),
    }
    permissions
}

pub(in crate::filesystem::sandbox) fn host_permissions(
    permissions: &cap_std::fs::Permissions,
    _file: &File,
) -> std::io::Result<std::fs::Permissions> {
    Ok(std::fs::Permissions::from_mode(permissions.mode()))
}

pub(in crate::filesystem::sandbox) fn sync_ordinary_descriptor<F: std::os::fd::AsFd + ?Sized>(
    file: &F,
) -> std::io::Result<()> {
    rustix::fs::fsync(file).map_err(std::io::Error::from)
}

pub(in crate::filesystem::sandbox) fn read_link_contents(
    directory: &cap_std::fs::Dir,
    name: &Path,
) -> std::io::Result<PathBuf> {
    directory.read_link_contents(name)
}

pub(in crate::filesystem::sandbox) fn create_host_symlink(
    link_target: &Path,
    link: &Path,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link_target, link)
}

pub(in crate::filesystem::sandbox) fn create_capability_symlink(
    directory: &cap_std::fs::Dir,
    link_target: &Path,
    link: &Path,
) -> std::io::Result<()> {
    directory.symlink_contents(link_target, link)
}
