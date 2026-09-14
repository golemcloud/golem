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

use crate::filesystem_pressure::FilesystemWriteRecovery;
pub use crate::sandbox_filesystem::FilesystemStorageError;
pub(crate) use crate::sandbox_filesystem::{FilesystemLimits, FilesystemSpace};
use crate::sandbox_filesystem::{
    FilesystemVolume, SandboxFilesystemProvisioning, observe_space_blocking,
};
use crate::services::golem_config::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, FilesystemStorageConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::OwnedAgentId;
use std::path::Path;

#[cfg(test)]
thread_local! {
    static BINDING_SPACE_OBSERVATION: std::cell::Cell<Option<FilesystemSpace>> = const {
        std::cell::Cell::new(None)
    };
}

mod lifecycle;

#[cfg(test)]
pub(crate) use lifecycle::tests::{
    billing_metered_resident_with_open_node_for_unload_test,
    metered_resident_with_open_node_for_unload_test, resident_for_unload_test,
};
pub(crate) use lifecycle::*;

const BYTES_PER_GIB: u128 = 1024 * 1024 * 1024;

fn observe_space_at_binding(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, FilesystemStorageError> {
    #[cfg(test)]
    if let Some(observation) = BINDING_SPACE_OBSERVATION.get() {
        return Ok(observation);
    }

    observe_space_blocking(volume)
}

impl FilesystemObjectLimitPolicyConfig {
    fn resolve(&self, allocated_bytes: u64) -> Result<FilesystemLimits, FilesystemStorageError> {
        if allocated_bytes == 0 {
            return Err(FilesystemStorageError::verification(
                "resolve nonzero agent filesystem storage limit",
                Path::new("<configuration>"),
            ));
        }

        let proportional = (u128::from(allocated_bytes) * u128::from(self.objects_per_gib()))
            .div_ceil(BYTES_PER_GIB);
        let proportional = u64::try_from(proportional).map_err(|_| {
            FilesystemStorageError::verification(
                "derive agent filesystem object limit",
                Path::new("<configuration>"),
            )
        })?;

        Ok(FilesystemLimits {
            allocated_bytes,
            filesystem_objects: proportional.clamp(self.minimum_objects(), self.maximum_objects()),
        })
    }
}

#[derive(Clone)]
pub(crate) struct AgentFilesystems {
    provisioning: SandboxFilesystemProvisioning,
    pressure: FilesystemPressureConfig,
    filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig,
}

impl AgentFilesystems {
    /// Binds filesystem provisioning and pressure settings for an executor.
    ///
    /// Callers create this service during executor startup, before any agent filesystem exists.
    /// Returns an error for invalid provisioning settings, failed volume observation, or a
    /// pressure target larger than the observed managed volume.
    pub(crate) fn new(settings: &FilesystemStorageConfig) -> Result<Self, FilesystemStorageError> {
        let provisioning = SandboxFilesystemProvisioning::new(
            settings.deterministic_root_dir.clone(),
            settings.managed_xfs_root_dir.clone(),
            settings.cleanup_retry.clone(),
        )?;
        let space = observe_space_at_binding(provisioning.volume())?;
        if let FilesystemSpace::Observed {
            total_bytes,
            total_filesystem_objects,
            ..
        } = space
        {
            settings
                .pressure
                .validate_capacity(total_bytes, total_filesystem_objects)?;
        }
        Ok(Self {
            provisioning,
            pressure: settings.pressure.clone(),
            filesystem_object_limit_policy: settings.filesystem_object_limit_policy.clone(),
        })
    }

    /// Returns the managed filesystem root that the file loader may use for its cache.
    ///
    /// Callers use this while wiring shared services, before agent creation. Unmanaged storage
    /// returns `None`; managed storage returns its root so cached sources stay on the same volume.
    pub(crate) fn initial_file_cache_root(&self) -> Option<&Path> {
        self.provisioning.initial_file_cache_root()
    }

    /// Returns the pressure thresholds used to recover writes on the provisioned volume.
    ///
    /// Worker creation uses this policy to build recovery for a new filesystem generation. This
    /// accessor has no lifecycle requirement and does not observe current capacity.
    pub(crate) fn pressure_policy(&self) -> &FilesystemPressureConfig {
        &self.pressure
    }

    /// Returns the volume shared by provisioned agent filesystems.
    ///
    /// Callers use the volume identity for capacity observation and pressure recovery before a
    /// generation is created. The returned value does not represent an individual agent target.
    pub(crate) fn volume(&self) -> &FilesystemVolume {
        self.provisioning.volume()
    }

    /// Resolves an agent's byte allocation into the limits installed on a new generation.
    ///
    /// Allocations at or above the resource service's effectively-unlimited sentinel produce
    /// `Unlimited`; smaller allocations also derive a bounded object limit. Zero or unrepresentable
    /// finite allocations return a verification error and must be rejected before creation.
    pub(crate) fn resolved_limits(
        &self,
        allocated_bytes: u64,
    ) -> Result<ResolvedStorageLimits, FilesystemStorageError> {
        if allocated_bytes >= AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE {
            Ok(ResolvedStorageLimits::Unlimited)
        } else {
            self.filesystem_object_limit_policy
                .resolve(allocated_bytes)
                .map(ResolvedStorageLimits::Finite)
        }
    }

    /// Creates an empty filesystem generation for an agent with the requested limits.
    ///
    /// Worker startup calls this before metering is bound or initial files are materialized. The
    /// returned filesystem is in the `Created` stage; provisioning failures are returned as
    /// `CreateFailure`, and write-capacity failures may use the supplied pressure recovery later.
    pub(crate) async fn create_fresh_with_pressure_recovery(
        &self,
        agent: OwnedAgentId,
        limits: ResolvedStorageLimits,
        pressure_recovery: FilesystemWriteRecovery,
    ) -> Result<CreatedFilesystem, CreateFailure> {
        lifecycle::create_fresh_with_pressure_recovery(
            self.provisioning.clone(),
            agent,
            limits,
            pressure_recovery,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    struct BindingSpaceObservationGuard(Option<FilesystemSpace>);

    impl Drop for BindingSpaceObservationGuard {
        fn drop(&mut self) {
            BINDING_SPACE_OBSERVATION.set(self.0);
        }
    }

    fn with_binding_space_observation<T>(space: FilesystemSpace, f: impl FnOnce() -> T) -> T {
        let previous = BINDING_SPACE_OBSERVATION.replace(Some(space));
        let _guard = BindingSpaceObservationGuard(previous);
        f()
    }

    #[test]
    fn object_limit_policy_resolves_floor_proportional_value_and_ceiling() {
        let policy = FilesystemObjectLimitPolicyConfig::new(32_768, 100, 50_000).unwrap();

        assert_eq!(policy.resolve(1).unwrap().filesystem_objects, 100);
        assert_eq!(
            policy
                .resolve(u64::try_from(BYTES_PER_GIB).unwrap())
                .unwrap()
                .filesystem_objects,
            32_768
        );
        assert_eq!(
            policy
                .resolve(u64::try_from(BYTES_PER_GIB * 10).unwrap())
                .unwrap()
                .filesystem_objects,
            50_000
        );
    }

    #[test]
    fn object_limit_policy_still_rejects_zero_allocated_bytes() {
        let error = FilesystemObjectLimitPolicyConfig::default()
            .resolve(0)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("resolve nonzero agent filesystem storage limit")
        );
    }

    #[test]
    fn registry_disk_sentinel_resolves_as_unlimited() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_bytes = settings.pressure.target_available_bytes();
        let filesystems = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: observed_total_bytes,
                available_bytes: observed_total_bytes,
                total_filesystem_objects: u64::MAX,
                available_filesystem_objects: u64::MAX,
            },
            || AgentFilesystems::new(&settings),
        )
        .unwrap();

        assert!(matches!(
            filesystems
                .resolved_limits(AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE - 1)
                .unwrap(),
            ResolvedStorageLimits::Finite(_)
        ));
        assert_eq!(
            filesystems
                .resolved_limits(AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE)
                .unwrap(),
            ResolvedStorageLimits::Unlimited
        );
        assert_eq!(
            filesystems.resolved_limits(u64::MAX).unwrap(),
            ResolvedStorageLimits::Unlimited
        );
    }

    #[test]
    fn agent_filesystems_binding_rejects_pressure_target_above_observed_capacity() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_bytes = settings.pressure.target_available_bytes() - 1;

        let result = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: observed_total_bytes,
                available_bytes: observed_total_bytes,
                total_filesystem_objects: u64::MAX,
                available_filesystem_objects: u64::MAX,
            },
            || AgentFilesystems::new(&settings),
        );

        let error = result
            .err()
            .expect("binding must reject a pressure target above observed capacity");
        assert!(
            error
                .to_string()
                .contains("fit filesystem pressure byte target within managed capacity")
        );
    }

    #[test]
    fn agent_filesystems_binding_accepts_pressure_target_equal_to_observed_capacity() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_bytes = settings.pressure.target_available_bytes();

        let result = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: observed_total_bytes,
                available_bytes: observed_total_bytes,
                total_filesystem_objects: u64::MAX,
                available_filesystem_objects: u64::MAX,
            },
            || AgentFilesystems::new(&settings),
        );

        if let Err(error) = result {
            panic!("binding rejected a pressure target equal to observed capacity: {error}");
        }
    }

    #[test]
    fn agent_filesystems_binding_rejects_object_pressure_target_above_observed_capacity() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_objects = settings.pressure.target_available_filesystem_objects() - 1;

        let result = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: u64::MAX,
                available_bytes: u64::MAX,
                total_filesystem_objects: observed_total_objects,
                available_filesystem_objects: observed_total_objects,
            },
            || AgentFilesystems::new(&settings),
        );

        let error = result
            .err()
            .expect("binding must reject an object pressure target above observed capacity");
        assert!(
            error
                .to_string()
                .contains("fit filesystem pressure object target within managed capacity")
        );
    }
}
