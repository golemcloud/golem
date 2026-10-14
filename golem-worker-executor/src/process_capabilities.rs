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

//! The check of `CAP_SYS_ADMIN` in the executor process.
//!
//! The deployment gives the executor its capabilities, and the executor does not change them. It
//! needs none: `xfs-quotad`, a separate process, makes the quota calls of `ManagedXfs` storage. The
//! executor reports `CAP_SYS_ADMIN` when the deployment gives it.

/// Whether the process holds `CAP_SYS_ADMIN` in its effective set. Always false on platforms
/// other than Linux.
pub fn holds_sys_admin() -> anyhow::Result<bool> {
    imp::holds_sys_admin()
}

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::Context;
    use rustix::thread::{CapabilitySet, capabilities};

    pub(super) fn holds_sys_admin() -> anyhow::Result<bool> {
        let current =
            capabilities(None).context("failed to read the capabilities of the process")?;
        Ok(current.effective.contains(CapabilitySet::SYS_ADMIN))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    pub(super) fn holds_sys_admin() -> anyhow::Result<bool> {
        Ok(false)
    }
}
