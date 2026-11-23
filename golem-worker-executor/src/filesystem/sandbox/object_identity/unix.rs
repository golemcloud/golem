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

use cap_fs_ext::MetadataExt as _;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::filesystem::sandbox) enum NativeFileIdentity {
    Unix {
        device: u64,
        inode: u64,
    },
    #[cfg(test)]
    Scripted(String),
}

pub(in crate::filesystem::sandbox) fn native_file_identity(
    metadata: &cap_std::fs::Metadata,
) -> std::io::Result<NativeFileIdentity> {
    Ok(NativeFileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}
