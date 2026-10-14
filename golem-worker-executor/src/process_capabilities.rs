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

//! `CAP_SYS_ADMIN` of the executor process.
//!
//! `ManagedXfs` storage sets and reads XFS project quotas, which Linux allows only with
//! `CAP_SYS_ADMIN`. The executor image gives the executor binary the file capability
//! `cap_sys_admin+p`: permitted, not effective. The kernel grants it only when the container's
//! bounding set holds `SYS_ADMIN` and `no_new_privs` is off. At startup, [`apply`] raises the
//! capability to effective in `ManagedXfs` mode, and removes it from every set in any other mode.
//!
//! Linux keeps capabilities per thread, and a new thread copies them from the thread that starts
//! it. [`apply`] must therefore run while the process has one thread, before the tracing, the
//! metrics and the Tokio runtime start theirs. It refuses to run otherwise.

use crate::services::golem_config::FilesystemStorageModeKind;

/// The documentation of the container settings that `ManagedXfs` needs.
pub const MANAGED_XFS_CONTAINER_DOCS: &str =
    "https://learn.golem.cloud/operate/filesystem_snapshots#run-the-executor-in-a-container";

/// What [`apply`] did to `CAP_SYS_ADMIN`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SysAdminCapability {
    /// `ManagedXfs`: the capability is permitted and effective.
    Raised,
    /// Another mode: the process held the capability, and no longer does.
    Dropped,
    /// Another mode: the process did not hold the capability. Also the result on every platform
    /// other than Linux.
    Absent,
}

/// Raises or drops `CAP_SYS_ADMIN` of the process for the filesystem storage `mode`.
pub fn apply(mode: FilesystemStorageModeKind) -> anyhow::Result<SysAdminCapability> {
    imp::apply(mode)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{MANAGED_XFS_CONTAINER_DOCS, SysAdminCapability};
    use crate::services::golem_config::FilesystemStorageModeKind;
    use anyhow::{Context, anyhow, bail};
    use rustix::thread::{CapabilitySet, CapabilitySets, capabilities, set_capabilities};

    pub(super) fn apply(mode: FilesystemStorageModeKind) -> anyhow::Result<SysAdminCapability> {
        let threads = std::fs::read_dir("/proc/self/task")
            .context("failed to count the threads of the executor process")?
            .count();
        if threads != 1 {
            bail!(
                "the executor sets its capabilities while it has one thread, but it has {threads}"
            );
        }
        let current =
            capabilities(None).context("failed to read the capabilities of the executor")?;
        let (target, outcome) = plan(mode, current)?;
        if target != current {
            set_capabilities(None, target)
                .context("failed to set the capabilities of the executor")?;
        }
        Ok(outcome)
    }

    /// The capability sets that the process needs for `mode`, given its `current` sets. Only
    /// `CAP_SYS_ADMIN` changes.
    pub(super) fn plan(
        mode: FilesystemStorageModeKind,
        current: CapabilitySets,
    ) -> anyhow::Result<(CapabilitySets, SysAdminCapability)> {
        let sys_admin = CapabilitySet::SYS_ADMIN;
        match mode {
            FilesystemStorageModeKind::ManagedXfs => {
                if !current.permitted.contains(sys_admin) {
                    return Err(anyhow!(
                        "ManagedXfs filesystem storage needs CAP_SYS_ADMIN, which the executor \
                         process is not permitted. The executor binary has the file capability \
                         cap_sys_admin+p, which Linux grants only when the container adds the \
                         SYS_ADMIN capability (Kubernetes: capabilities.add: [SYS_ADMIN]; Docker: \
                         --cap-add SYS_ADMIN) and allows privilege escalation (Kubernetes: \
                         allowPrivilegeEscalation: true; Docker: no \
                         --security-opt no-new-privileges). See {MANAGED_XFS_CONTAINER_DOCS}"
                    ));
                }
                Ok((
                    CapabilitySets {
                        effective: current.effective | sys_admin,
                        ..current
                    },
                    SysAdminCapability::Raised,
                ))
            }
            FilesystemStorageModeKind::Temporary
            | FilesystemStorageModeKind::Directory
            | FilesystemStorageModeKind::ReflinkXfs
            | FilesystemStorageModeKind::Apfs => {
                let held = current.effective | current.permitted | current.inheritable;
                if !held.contains(sys_admin) {
                    return Ok((current, SysAdminCapability::Absent));
                }
                Ok((
                    CapabilitySets {
                        effective: current.effective - sys_admin,
                        permitted: current.permitted - sys_admin,
                        inheritable: current.inheritable - sys_admin,
                    },
                    SysAdminCapability::Dropped,
                ))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{SysAdminCapability, plan};
        use crate::services::golem_config::FilesystemStorageModeKind;
        use assert2::{assert, let_assert};
        use rustix::thread::{CapabilitySet, CapabilitySets};
        use test_r::test;

        const OTHER_MODES: [FilesystemStorageModeKind; 4] = [
            FilesystemStorageModeKind::Temporary,
            FilesystemStorageModeKind::Directory,
            FilesystemStorageModeKind::ReflinkXfs,
            FilesystemStorageModeKind::Apfs,
        ];

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

        const NONE: CapabilitySet = CapabilitySet::empty();
        const SYS_ADMIN: CapabilitySet = CapabilitySet::SYS_ADMIN;

        #[test]
        fn managed_xfs_raises_a_permitted_capability_to_effective() {
            let current = sets(NONE, SYS_ADMIN, NONE);
            let_assert!(
                Ok((target, outcome)) = plan(FilesystemStorageModeKind::ManagedXfs, current)
            );
            assert!(target == sets(SYS_ADMIN, SYS_ADMIN, NONE));
            assert!(outcome == SysAdminCapability::Raised);
        }

        #[test]
        fn managed_xfs_keeps_the_other_capabilities() {
            let others = CapabilitySet::CHOWN | CapabilitySet::NET_BIND_SERVICE;
            let current = sets(others, others | SYS_ADMIN, CapabilitySet::KILL);
            let_assert!(Ok((target, _)) = plan(FilesystemStorageModeKind::ManagedXfs, current));
            assert!(target == sets(others | SYS_ADMIN, others | SYS_ADMIN, CapabilitySet::KILL));
        }

        #[test]
        fn managed_xfs_without_the_permitted_capability_names_the_settings() {
            for current in [
                sets(NONE, NONE, NONE),
                sets(NONE, NONE, SYS_ADMIN),
                sets(CapabilitySet::CHOWN, CapabilitySet::CHOWN, NONE),
            ] {
                let_assert!(Err(error) = plan(FilesystemStorageModeKind::ManagedXfs, current));
                let message = error.to_string();
                assert!(message.contains("CAP_SYS_ADMIN"));
                assert!(message.contains("capabilities.add: [SYS_ADMIN]"));
                assert!(message.contains("--cap-add SYS_ADMIN"));
                assert!(message.contains("allowPrivilegeEscalation: true"));
                assert!(message.contains("no-new-privileges"));
                assert!(message.contains(super::MANAGED_XFS_CONTAINER_DOCS));
            }
        }

        #[test]
        fn other_modes_drop_the_capability_from_every_set() {
            let others = CapabilitySet::CHOWN | CapabilitySet::NET_BIND_SERVICE;
            for mode in OTHER_MODES {
                for current in [
                    sets(NONE, SYS_ADMIN, NONE),
                    sets(SYS_ADMIN, SYS_ADMIN, NONE),
                    sets(NONE, NONE, SYS_ADMIN),
                    sets(others | SYS_ADMIN, others | SYS_ADMIN, others | SYS_ADMIN),
                ] {
                    let_assert!(Ok((target, outcome)) = plan(mode, current));
                    assert!(target.effective == current.effective - SYS_ADMIN);
                    assert!(target.permitted == current.permitted - SYS_ADMIN);
                    assert!(target.inheritable == current.inheritable - SYS_ADMIN);
                    assert!(outcome == SysAdminCapability::Dropped);
                }
            }
        }

        #[test]
        fn other_modes_without_the_capability_change_nothing() {
            let others = CapabilitySet::CHOWN | CapabilitySet::NET_BIND_SERVICE;
            for mode in OTHER_MODES {
                for current in [sets(NONE, NONE, NONE), sets(others, others, others)] {
                    let_assert!(Ok((target, outcome)) = plan(mode, current));
                    assert!(target == current);
                    assert!(outcome == SysAdminCapability::Absent);
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::SysAdminCapability;
    use crate::services::golem_config::FilesystemStorageModeKind;

    pub(super) fn apply(_mode: FilesystemStorageModeKind) -> anyhow::Result<SysAdminCapability> {
        Ok(SysAdminCapability::Absent)
    }
}
