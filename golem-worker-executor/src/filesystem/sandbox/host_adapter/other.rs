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

use super::SandboxFilesystem;
use crate::filesystem::errors::OperationContext;
use crate::filesystem::sandbox::{FilesystemStorageError, InstalledLimits};
use crate::filesystem::volume::{FilesystemAllocation, FilesystemLimits};
use std::num::{NonZeroU32, NonZeroU64};

/// Who enforces and measures the limits of a sandbox. Project quotas are unavailable here, but
/// shared copy policies can describe a project without installing one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::filesystem::sandbox) enum QuotaAuthority {
    Unsupported,
    #[expect(dead_code, reason = "project quotas are installed only on Linux")]
    Project {
        project_id: NonZeroU32,
        filesystem_block_bytes: NonZeroU64,
    },
}

impl SandboxFilesystem {
    pub(in crate::filesystem::sandbox) async fn observe_project_allocation(
        &self,
    ) -> Result<Option<FilesystemAllocation>, FilesystemStorageError> {
        match self.quota_authority {
            QuotaAuthority::Unsupported => Ok(None),
            QuotaAuthority::Project { .. } => {
                unreachable!("managed XFS is unavailable on this platform")
            }
        }
    }

    pub(in crate::filesystem::sandbox) async fn install_storage_limits(
        &self,
        _limits: FilesystemLimits,
    ) -> Result<InstalledLimits, FilesystemStorageError> {
        match self.quota_authority {
            QuotaAuthority::Unsupported => Err(FilesystemStorageError::from(
                OperationContext::verification(
                    "install limits without quota authority",
                    Some(self.root()),
                ),
            )),
            QuotaAuthority::Project { .. } => {
                unreachable!("managed XFS is unavailable on this platform")
            }
        }
    }
}
