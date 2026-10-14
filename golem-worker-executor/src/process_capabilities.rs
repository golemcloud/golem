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

//! Checks of `CAP_SYS_ADMIN` in the executor and in its XFS quota helper.
//!
//! The deployment gives each process its capabilities; neither process changes them. Only the XFS
//! quota helper of `ManagedXfs` storage needs a capability, `CAP_SYS_ADMIN`, and refuses to start
//! without it. The executor needs none, and reports `CAP_SYS_ADMIN` when the deployment gives it.

/// Whether the process holds `CAP_SYS_ADMIN` in its effective set. Always false on platforms
/// other than Linux.
pub fn holds_sys_admin() -> anyhow::Result<bool> {
    imp::holds_sys_admin()
}

/// Fails, naming the settings to add, when the XFS quota helper does not hold `CAP_SYS_ADMIN` in
/// its effective set.
#[cfg(target_os = "linux")]
pub(crate) fn require_sys_admin_for_quota_helper() -> anyhow::Result<()> {
    if imp::holds_sys_admin()? {
        Ok(())
    } else {
        Err(missing_sys_admin())
    }
}

#[cfg(target_os = "linux")]
fn missing_sys_admin() -> anyhow::Error {
    anyhow::anyhow!(
        "the XFS quota helper needs CAP_SYS_ADMIN, which this process does not hold. Run it with \
         that capability: as root with only SYS_ADMIN added (Kubernetes: runAsUser: 0, \
         capabilities: {{drop: [ALL], add: [SYS_ADMIN]}}; Docker: --user 0 --cap-drop ALL \
         --cap-add SYS_ADMIN), or as a systemd service with AmbientCapabilities=CAP_SYS_ADMIN. \
         See {}",
        crate::sandbox_filesystem::xfs::quota_helper::HELPER_DOCS
    )
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use assert2::assert;
    use test_r::test;

    #[test]
    fn the_missing_capability_error_names_the_settings() {
        let message = super::missing_sys_admin().to_string();
        assert!(message.contains("CAP_SYS_ADMIN"));
        assert!(message.contains("add: [SYS_ADMIN]"));
        assert!(message.contains("--cap-add SYS_ADMIN"));
        assert!(message.contains("AmbientCapabilities=CAP_SYS_ADMIN"));
        assert!(message.contains("run-the-xfs-quota-helper"));
    }
}
