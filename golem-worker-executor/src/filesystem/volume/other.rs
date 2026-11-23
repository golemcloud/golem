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

use super::{FilesystemSpace, FilesystemVolume, StorageVolumeError};
use crate::filesystem::errors::OperationContext;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct PlatformState {}

#[derive(Clone)]
pub(super) struct CopyOnWrite;

impl CopyOnWrite {
    pub(super) fn root(&self) -> Option<&Arc<File>> {
        None
    }

    pub(super) fn observe_space(&self) -> std::io::Result<FilesystemSpace> {
        unreachable!("copy-on-write storage is unavailable on this platform")
    }
}

impl FilesystemVolume {
    pub(super) fn open_apfs(root: &Path) -> Result<Self, StorageVolumeError> {
        Err(StorageVolumeError::from(OperationContext::verification(
            "initialize APFS storage on a non-macOS platform",
            Some(root),
        )))
    }
}

impl PlatformState {
    pub(super) fn verify_host_directory(&self, _path: &Path) -> Result<(), StorageVolumeError> {
        Ok(())
    }
}

#[cfg(test)]
mod test_support {
    use super::*;

    impl FilesystemVolume {
        pub(crate) fn copy_on_write(_root: Arc<File>) -> Self {
            Self::from(CopyOnWrite)
        }
    }

    pub(crate) fn copy_on_write_volume_for_test(root: Arc<File>) -> FilesystemVolume {
        FilesystemVolume::copy_on_write(root)
    }
}
#[cfg(test)]
pub(crate) use test_support::copy_on_write_volume_for_test;
