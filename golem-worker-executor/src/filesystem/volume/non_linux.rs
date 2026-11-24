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

use super::{FilesystemVolume, StorageVolumeError};
use crate::filesystem::errors::OperationContext;
use golem_common::model::RetryConfig;
use std::path::Path;

impl FilesystemVolume {
    pub(super) fn open_managed(
        root: &Path,
        _socket: &Path,
        _retry: &RetryConfig,
    ) -> Result<Self, StorageVolumeError> {
        Self::open_reflink(root)
    }

    pub(super) fn open_reflink(root: &Path) -> Result<Self, StorageVolumeError> {
        Err(StorageVolumeError::from(OperationContext::verification(
            "initialize XFS storage on a non-Linux platform",
            Some(root),
        )))
    }
}
