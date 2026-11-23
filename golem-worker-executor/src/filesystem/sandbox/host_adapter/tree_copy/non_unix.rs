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

/// Gives `permissions` the write permission that `access` sets.
pub(in crate::filesystem::sandbox) fn seeded_permissions(
    mut permissions: std::fs::Permissions,
    access: SeedAccess,
) -> std::fs::Permissions {
    match access {
        SeedAccess::FromSource => {}
        SeedAccess::ReadOnly => permissions.set_readonly(true),
        SeedAccess::ReadWrite => permissions.set_readonly(false),
    }
    permissions
}

pub(in crate::filesystem::sandbox) fn host_permissions(
    permissions: &cap_std::fs::Permissions,
    file: &File,
) -> std::io::Result<std::fs::Permissions> {
    let mut host = file.metadata()?.permissions();
    host.set_readonly(permissions.readonly());
    Ok(host)
}

pub(in crate::filesystem::sandbox) fn sync_ordinary_descriptor<F: ?Sized>(
    _file: &F,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "ordinary filesystem sync is unavailable on this platform",
    ))
}

pub(in crate::filesystem::sandbox) fn read_link_contents(
    directory: &cap_std::fs::Dir,
    name: &Path,
) -> std::io::Result<PathBuf> {
    directory.read_link(name)
}

pub(in crate::filesystem::sandbox) fn create_host_symlink(
    _link_target: &Path,
    _link: &Path,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "tree copy cannot recreate symlinks on this platform",
    ))
}

pub(in crate::filesystem::sandbox) fn create_capability_symlink(
    _directory: &cap_std::fs::Dir,
    _link_target: &Path,
    _link: &Path,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "tree copy cannot recreate symlinks on this platform",
    ))
}
