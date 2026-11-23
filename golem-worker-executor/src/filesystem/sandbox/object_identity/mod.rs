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

use std::fmt::Debug;
use std::hash::Hash;

#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(unix, windows)))]
#[path = "other.rs"]
mod platform;

pub(in crate::filesystem::sandbox) use platform::{NativeFileIdentity, native_file_identity};

#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct SandboxDirectoryId(pub(in crate::filesystem::sandbox) NativeFileIdentity);

/// The identity of one filesystem object in a sandbox.
///
/// Two values are equal when they identify the same object. The identity of a deleted object can
/// identify a new object later.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SandboxObjectId(pub(in crate::filesystem::sandbox) NativeFileIdentity);

pub(in crate::filesystem::sandbox) fn directory_identity(
    directory: &cap_std::fs::Dir,
) -> std::io::Result<SandboxDirectoryId> {
    let metadata = directory.dir_metadata()?;
    Ok(SandboxDirectoryId(native_file_identity(&metadata)?))
}
