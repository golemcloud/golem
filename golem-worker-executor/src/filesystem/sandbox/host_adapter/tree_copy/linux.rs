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

use super::{File, NonZeroU32, Path, SeedSyncPlatform};
use crate::filesystem::volume::{
    clone_file_contents, clone_file_contents_into_project, sync_volume,
};

pub(in crate::filesystem::sandbox) use super::non_macos::*;

pub(in crate::filesystem::sandbox) fn native_seed_sync_platform() -> SeedSyncPlatform {
    SeedSyncPlatform::Linux
}

pub(in crate::filesystem::sandbox) fn sync_seed_linux_volume(path: &Path) -> std::io::Result<()> {
    sync_volume(path)
}

pub(in crate::filesystem::sandbox) fn reflink_file(
    source: &File,
    target: &File,
    project: Option<NonZeroU32>,
) -> std::io::Result<()> {
    match project {
        Some(project) => clone_file_contents_into_project(source, target, project),
        None => clone_file_contents(source, target),
    }
}
