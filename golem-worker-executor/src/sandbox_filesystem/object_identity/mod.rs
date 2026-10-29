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
use std::fmt::Debug;
use std::hash::Hash;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::sandbox_filesystem) enum NativeFileIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows {
        volume_serial_number: u32,
        file_index: u64,
    },
    #[cfg(test)]
    Scripted(String),
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct SandboxDirectoryId(pub(in crate::sandbox_filesystem) NativeFileIdentity);

/// The identity of one filesystem object in a sandbox.
///
/// Two values are equal when they identify the same object. The identity of a deleted object can
/// identify a new object later.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SandboxObjectId(pub(in crate::sandbox_filesystem) NativeFileIdentity);

pub(in crate::sandbox_filesystem) fn directory_identity(
    directory: &cap_std::fs::Dir,
) -> std::io::Result<SandboxDirectoryId> {
    let metadata = directory.dir_metadata()?;
    Ok(SandboxDirectoryId(native_file_identity(&metadata)?))
}

pub(in crate::sandbox_filesystem) fn native_file_identity(
    metadata: &cap_std::fs::Metadata,
) -> std::io::Result<NativeFileIdentity> {
    #[cfg(unix)]
    return Ok(NativeFileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    });
    #[cfg(windows)]
    return Ok(NativeFileIdentity::Windows {
        volume_serial_number: u32::try_from(metadata.dev()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "native volume serial number does not fit its opaque identity",
            )
        })?,
        file_index: metadata.ino(),
    });
    #[cfg(not(any(unix, windows)))]
    return Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native file identity is unavailable on this target",
    ));
}
