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

use super::{CapabilityTempFile, File, Path};

pub(in crate::filesystem::sandbox) fn clone_seed_file<'a>(
    _parent: &'a cap_std::fs::Dir,
    _source: &File,
) -> std::io::Result<CapabilityTempFile<'a>> {
    Err(macos_clone_unavailable())
}

pub(in crate::filesystem::sandbox) fn clone_copied_file(
    _source: &File,
    _target: &Path,
) -> std::io::Result<File> {
    Err(macos_clone_unavailable())
}

pub(in crate::filesystem::sandbox) fn macos_clone_unavailable() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "macOS file clone is unavailable on this platform",
    )
}

pub(in crate::filesystem::sandbox) fn sync_seed_mac_full_flush(
    _directory: &cap_std::fs::Dir,
    _seed_flusher: &cap_std::fs::File,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "macOS fullflush is unavailable on this platform",
    ))
}
