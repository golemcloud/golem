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

use super::{File, NonZeroU32, Path};

pub(in crate::filesystem::sandbox) fn sync_seed_linux_volume(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Linux seed volume sync is unavailable on this platform",
    ))
}

pub(in crate::filesystem::sandbox) fn reflink_file(
    _source: &File,
    _target: &File,
    _project: Option<NonZeroU32>,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Linux file-descriptor cloning is unavailable on this platform",
    ))
}
