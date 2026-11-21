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

use super::macos_finalization::{open_cloned_file, sync_seed_batch_submissions};
use super::{CapabilityTempFile, File, Path, SeedSyncPlatform};
use super::{sync_ordinary_descriptor, temporary_copy_name};
use crate::filesystem::volume::clone_file_at;
use cap_fs_ext::OpenOptionsFollowExt as _;

pub(in crate::filesystem::sandbox) use super::non_linux::*;

pub(in crate::filesystem::sandbox) fn native_seed_sync_platform() -> SeedSyncPlatform {
    SeedSyncPlatform::MacOs
}

impl<'a> CapabilityTempFile<'a> {
    fn from_clone(directory: &'a cap_std::fs::Dir, source: &File) -> std::io::Result<Self> {
        let name = std::iter::repeat_with(temporary_copy_name)
            .find_map(|name| match clone_file_at(source, directory, &name) {
                Ok(()) => Some(Ok(name)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            })
            .expect("an unbounded clone-name iterator must return a result")?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(cap_fs_ext::FollowSymlinks::No);
        let file = open_cloned_file(
            || directory.open_with(&name, &options),
            || directory.remove_file(&name),
        )?;
        Ok(Self {
            directory,
            name: Some(name),
            file,
        })
    }
}

pub(in crate::filesystem::sandbox) fn clone_seed_file<'a>(
    parent: &'a cap_std::fs::Dir,
    source: &File,
) -> std::io::Result<CapabilityTempFile<'a>> {
    CapabilityTempFile::from_clone(parent, source)
}

pub(in crate::filesystem::sandbox) fn clone_copied_file(
    source: &File,
    target: &Path,
) -> std::io::Result<File> {
    let parent = File::open(
        target
            .parent()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
    )?;
    clone_file_at(
        source,
        &parent,
        target
            .file_name()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?
            .as_ref(),
    )?;
    File::open(target)
}

pub(in crate::filesystem::sandbox) fn sync_seed_mac_full_flush(
    directory: &cap_std::fs::Dir,
    seed_flusher: &cap_std::fs::File,
) -> std::io::Result<()> {
    sync_seed_batch_submissions(
        || sync_ordinary_descriptor(seed_flusher),
        || sync_ordinary_descriptor(directory),
        || rustix::fs::fcntl_fullfsync(seed_flusher).map_err(std::io::Error::from),
    )
}
