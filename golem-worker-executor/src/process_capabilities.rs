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

//! The capabilities of the executor process and of its XFS quota helper.
//!
//! The executor image gives the executor binary the file capability `cap_sys_admin+p`:
//! permitted, not effective. The kernel grants it only when the container's bounding set holds
//! `SYS_ADMIN` and `no_new_privs` is off. Only the XFS quota helper, which runs no guest code,
//! uses it: [`retain_only_sys_admin`] keeps it there and removes every other capability. The
//! executor removes it with [`restrict_executor`] once it started the helper, and sets
//! `no_new_privs`, so that no later `exec` gains a privilege.
//!
//! Linux keeps capabilities per thread, and a new thread copies them from the thread that starts
//! it. Both functions must therefore run while the process has one thread, and refuse to run
//! otherwise.

/// The documentation of the container settings that `ManagedXfs` needs.
pub const MANAGED_XFS_CONTAINER_DOCS: &str =
    "https://learn.golem.cloud/operate/filesystem_snapshots#run-the-executor-in-a-container";

/// What [`restrict_executor`] did to `CAP_SYS_ADMIN`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SysAdminCapability {
    /// The process held the capability, and no longer does.
    Dropped,
    /// The process did not hold the capability. Also the result on every platform other than
    /// Linux.
    Absent,
}

/// Removes `CAP_SYS_ADMIN` from every capability set of the executor process, and sets
/// `no_new_privs`.
pub fn restrict_executor() -> anyhow::Result<SysAdminCapability> {
    imp::restrict_executor()
}

/// Keeps `CAP_SYS_ADMIN` as the only capability of the XFS quota helper, permitted and effective,
/// and sets `no_new_privs`. Fails, naming the settings to add, when the capability is not
/// permitted.
#[cfg(target_os = "linux")]
pub(crate) fn retain_only_sys_admin() -> anyhow::Result<()> {
    imp::retain_only_sys_admin()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{MANAGED_XFS_CONTAINER_DOCS, SysAdminCapability};
    use anyhow::{Context, anyhow, bail};
    use rustix::thread::{CapabilitySet, CapabilitySets, capabilities, set_capabilities};

    pub(super) fn restrict_executor() -> anyhow::Result<SysAdminCapability> {
        require_one_thread()?;
        let current = read()?;
        let (target, outcome) = executor_sets(current);
        write(current, target)?;
        rustix::thread::set_no_new_privs(true)
            .context("failed to set no_new_privs for the executor")?;
        Ok(outcome)
    }

    pub(super) fn retain_only_sys_admin() -> anyhow::Result<()> {
        require_one_thread()?;
        let current = read()?;
        let target = helper_sets(current)?;
        write(current, target)?;
        rustix::thread::set_no_new_privs(true)
            .context("failed to set no_new_privs for the XFS quota helper")?;
        Ok(())
    }

    fn require_one_thread() -> anyhow::Result<()> {
        let threads = std::fs::read_dir("/proc/self/task")
            .context("failed to count the threads of the process")?
            .count();
        if threads != 1 {
            bail!(
                "the process sets its capabilities while it has one thread, but it has {threads}"
            );
        }
        Ok(())
    }

    fn read() -> anyhow::Result<CapabilitySets> {
        capabilities(None).context("failed to read the capabilities of the process")
    }

    fn write(current: CapabilitySets, target: CapabilitySets) -> anyhow::Result<()> {
        if target != current {
            set_capabilities(None, target)
                .context("failed to set the capabilities of the process")?;
        }
        Ok(())
    }

    /// The capability sets of the executor: `current` without `CAP_SYS_ADMIN`.
    pub(super) fn executor_sets(current: CapabilitySets) -> (CapabilitySets, SysAdminCapability) {
        let sys_admin = CapabilitySet::SYS_ADMIN;
        let held = current.effective | current.permitted | current.inheritable;
        if !held.contains(sys_admin) {
            return (current, SysAdminCapability::Absent);
        }
        (
            CapabilitySets {
                effective: current.effective - sys_admin,
                permitted: current.permitted - sys_admin,
                inheritable: current.inheritable - sys_admin,
            },
            SysAdminCapability::Dropped,
        )
    }

    /// The capability sets of the quota helper: only `CAP_SYS_ADMIN`, permitted and effective.
    pub(super) fn helper_sets(current: CapabilitySets) -> anyhow::Result<CapabilitySets> {
        let sys_admin = CapabilitySet::SYS_ADMIN;
        if !current.permitted.contains(sys_admin) {
            return Err(anyhow!(
                "ManagedXfs filesystem storage needs CAP_SYS_ADMIN for its XFS quota helper, \
                 which the process is not permitted. The executor binary has the file capability \
                 cap_sys_admin+p, which Linux grants only when the container adds the SYS_ADMIN \
                 capability (Kubernetes: capabilities.add: [SYS_ADMIN]; Docker: --cap-add \
                 SYS_ADMIN) and allows privilege escalation (Kubernetes: \
                 allowPrivilegeEscalation: true; Docker: no --security-opt no-new-privileges). \
                 See {MANAGED_XFS_CONTAINER_DOCS}"
            ));
        }
        Ok(CapabilitySets {
            effective: sys_admin,
            permitted: sys_admin,
            inheritable: CapabilitySet::empty(),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{SysAdminCapability, executor_sets, helper_sets};
        use assert2::{assert, let_assert};
        use rustix::thread::{CapabilitySet, CapabilitySets};
        use test_r::test;

        const NONE: CapabilitySet = CapabilitySet::empty();
        const SYS_ADMIN: CapabilitySet = CapabilitySet::SYS_ADMIN;

        fn sets(
            effective: CapabilitySet,
            permitted: CapabilitySet,
            inheritable: CapabilitySet,
        ) -> CapabilitySets {
            CapabilitySets {
                effective,
                permitted,
                inheritable,
            }
        }

        fn others() -> CapabilitySet {
            CapabilitySet::CHOWN | CapabilitySet::NET_BIND_SERVICE
        }

        #[test]
        fn the_executor_drops_the_capability_from_every_set() {
            for current in [
                sets(NONE, SYS_ADMIN, NONE),
                sets(SYS_ADMIN, SYS_ADMIN, NONE),
                sets(NONE, NONE, SYS_ADMIN),
                sets(
                    others() | SYS_ADMIN,
                    others() | SYS_ADMIN,
                    others() | SYS_ADMIN,
                ),
            ] {
                let (target, outcome) = executor_sets(current);
                assert!(target.effective == current.effective - SYS_ADMIN);
                assert!(target.permitted == current.permitted - SYS_ADMIN);
                assert!(target.inheritable == current.inheritable - SYS_ADMIN);
                assert!(outcome == SysAdminCapability::Dropped);
            }
        }

        #[test]
        fn the_executor_without_the_capability_changes_nothing() {
            for current in [sets(NONE, NONE, NONE), sets(others(), others(), others())] {
                let (target, outcome) = executor_sets(current);
                assert!(target == current);
                assert!(outcome == SysAdminCapability::Absent);
            }
        }

        #[test]
        fn the_helper_keeps_only_the_permitted_capability() {
            for current in [
                sets(NONE, SYS_ADMIN, NONE),
                sets(SYS_ADMIN, SYS_ADMIN, NONE),
                sets(
                    others() | SYS_ADMIN,
                    others() | SYS_ADMIN,
                    others() | SYS_ADMIN,
                ),
            ] {
                let_assert!(Ok(target) = helper_sets(current));
                assert!(target == sets(SYS_ADMIN, SYS_ADMIN, NONE));
            }
        }

        #[test]
        fn the_helper_without_the_permitted_capability_names_the_settings() {
            for current in [
                sets(NONE, NONE, NONE),
                sets(NONE, NONE, SYS_ADMIN),
                sets(others(), others(), NONE),
            ] {
                let_assert!(Err(error) = helper_sets(current));
                let message = error.to_string();
                assert!(message.contains("CAP_SYS_ADMIN"));
                assert!(message.contains("capabilities.add: [SYS_ADMIN]"));
                assert!(message.contains("--cap-add SYS_ADMIN"));
                assert!(message.contains("allowPrivilegeEscalation: true"));
                assert!(message.contains("no-new-privileges"));
                assert!(message.contains(super::MANAGED_XFS_CONTAINER_DOCS));
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::SysAdminCapability;

    pub(super) fn restrict_executor() -> anyhow::Result<SysAdminCapability> {
        Ok(SysAdminCapability::Absent)
    }
}
