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
    FilesystemVolume, HostDirectory, SandboxFilesystemProvisioning, observe_space_blocking,
};
use crate::services::golem_config::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, FilesystemStorageConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::OwnedAgentId;
use std::path::Path;
use std::sync::Arc;

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
    metered_resident_with_open_node_for_unload_test, no_initial_files, resident_for_unload_test,
    scratch_directory,
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

/// The owner write bit of a file mode creation mask.
#[cfg(unix)]
const OWNER_WRITE_BIT: libc::mode_t = 0o200;

/// Clears bit 0o200 of the file mode creation mask of the process, and keeps the other bits.
///
/// Each file that an agent creates then has write permission for its owner. The initial-file rule
/// counts a file without write permission at a read-only declared path as Golem's file when its
/// content equals the declaration, so a file of an agent must always have this permission. The
/// call is idempotent, and it changes only the owner write bit of the mask.
///
/// On Linux the function reads the current mask from the `Umask:` line of
/// `/proc/thread-self/status`. That read does not change the mask. On other Unix platforms, and on
/// Linux when the line is not available, the function sets the mask two times: to 0o022, which
/// gives the current mask, and then to that mask without the owner write bit. Between the two
/// calls, a file that another thread creates gets the usual permissions of the mask 0o022.
///
/// Windows has no file mode creation mask. There, a file is read-only only when its read-only
/// attribute is set, and an agent cannot set that attribute, so the service changes nothing.
#[cfg(unix)]
fn keep_owner_write_permission() {
    let mask = current_file_creation_mask();
    // SAFETY: `umask` only replaces the mask of the process. It cannot fail.
    unsafe {
        libc::umask(mask_without_owner_write(mask));
    }
}

/// Gives the current file mode creation mask of the calling thread. Where
/// `/proc/thread-self/status` has no `Umask:` line, the mask is 0o022 after the call.
///
/// The function reads `/proc/thread-self/status`, not `/proc/self/status`. `/proc/self` is the
/// thread group leader, and a thread that does not share the filesystem attributes of the process
/// has its own mask. All threads of the executor share one mask, so there the read gives the mask
/// of the process.
#[cfg(target_os = "linux")]
fn current_file_creation_mask() -> libc::mode_t {
    std::fs::read_to_string("/proc/thread-self/status")
        .ok()
        .as_deref()
        .and_then(status_umask)
        .unwrap_or_else(replaced_file_creation_mask)
}

/// Gives the current file mode creation mask of the process. The mask is 0o022 after the call.
#[cfg(all(unix, not(target_os = "linux")))]
fn current_file_creation_mask() -> libc::mode_t {
    replaced_file_creation_mask()
}

/// Sets the file mode creation mask of the process to 0o022, and gives the mask before the call.
///
/// The probe mask is 0o022, not 0o777. Other services of the process can create files at the same
/// time, such as a database file or a log file. With 0o777 such a file gets mode 000 and fails at
/// random. With 0o022 it gets the usual permissions for that moment.
#[cfg(unix)]
fn replaced_file_creation_mask() -> libc::mode_t {
    // SAFETY: `umask` only replaces the mask of the process. It cannot fail.
    unsafe { libc::umask(0o022) }
}

/// Gives `mask` without the owner write bit. Every other bit stays as it is.
#[cfg(unix)]
fn mask_without_owner_write(mask: libc::mode_t) -> libc::mode_t {
    mask & !OWNER_WRITE_BIT
}

/// Gives the file mode creation mask on the `Umask:` line of a `/proc/<pid>/status` text.
#[cfg(target_os = "linux")]
fn status_umask(status: &str) -> Option<libc::mode_t> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Umask:"))
        .and_then(|value| libc::mode_t::from_str_radix(value.trim(), 8).ok())
}

#[derive(Clone)]
pub(crate) struct AgentFilesystems {
    provisioning: SandboxFilesystemProvisioning,
    pressure: FilesystemPressureConfig,
    filesystem_object_limit_policy: FilesystemObjectLimitPolicyConfig,
    scratch: Arc<HostDirectory>,
}

impl AgentFilesystems {
    /// Binds filesystem provisioning and pressure settings for an executor, and makes the
    /// `.scratch` host directory for captures and restores.
    ///
    /// Callers create this service during executor startup, before any agent filesystem exists.
    /// Every embedder of the executor starts the service here. On Unix platforms the service clears
    /// bit 0o200 of the process umask and keeps the other bits, so each file that an agent creates
    /// has write permission for its owner. Making `.scratch` removes what an earlier process left
    /// under the name. Returns an error for invalid provisioning settings, failed volume observation, a
    /// pressure target larger than the observed managed volume, or a `.scratch` directory that
    /// cannot be made.
    pub(crate) async fn new(
        settings: &FilesystemStorageConfig,
    ) -> Result<Self, FilesystemStorageError> {
        #[cfg(unix)]
        keep_owner_write_permission();
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
        let scratch =
            HostDirectory::create_at_root(&provisioning, std::ffi::OsStr::new(".scratch")).await?;
        Ok(Self {
            provisioning,
            pressure: settings.pressure.clone(),
            filesystem_object_limit_policy: settings.filesystem_object_limit_policy.clone(),
            scratch: Arc::new(scratch),
        })
    }

    /// Returns the provisioning that makes agent filesystems and host directories on the volume.
    ///
    /// Callers use this while wiring shared services, before agent creation.
    pub(crate) fn provisioning(&self) -> &SandboxFilesystemProvisioning {
        &self.provisioning
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
            Arc::clone(&self.scratch),
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

    #[cfg(unix)]
    #[test]
    fn the_file_mode_creation_mask_loses_only_the_owner_write_bit() {
        [
            (0o022, 0o022),
            (0o077, 0o077),
            (0o222, 0o022),
            (0o277, 0o077),
        ]
        .into_iter()
        .for_each(|(mask, expected)| {
            assert_eq!(
                mask_without_owner_write(mask),
                expected,
                "the mask {mask:o} must become {expected:o}"
            );
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_file_mode_creation_mask_comes_from_the_umask_line_of_a_status_text() {
        assert_eq!(
            status_umask("Name:\tworker-executor\nUmask:\t0277\nState:\tR (running)\n"),
            Some(0o277)
        );
        assert_eq!(
            status_umask("Name:\tworker-executor\nState:\tR (running)\n"),
            None
        );
    }

    /// Runs `f` on a new thread whose file mode creation mask is `mask`, and gives its result.
    ///
    /// The file mode creation mask is a value that all threads of a process share, and other
    /// tests of this binary change it through `AgentFilesystems::new`. So the thread first takes
    /// its own copy of the filesystem attributes of the process with `unshare(CLONE_FS)`. After
    /// that call, a change of the mask on the thread changes only the mask of the thread.
    #[cfg(target_os = "linux")]
    fn with_private_file_creation_mask<T: Send + 'static>(
        mask: libc::mode_t,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        std::thread::spawn(move || {
            // SAFETY: `unshare` with `CLONE_FS` only gives the calling thread its own copy of the
            // root directory, the current directory and the file mode creation mask. It needs no
            // privilege.
            let unshared = unsafe { libc::unshare(libc::CLONE_FS) };
            assert_eq!(
                unshared,
                0,
                "the thread must get its own filesystem attributes: {}",
                std::io::Error::last_os_error()
            );
            // SAFETY: `umask` only replaces the mask of the thread. It cannot fail.
            unsafe {
                libc::umask(mask);
            }
            f()
        })
        .join()
        .expect("the thread with a private file mode creation mask must not panic")
    }

    /// Gives the file mode creation mask of the calling thread, which must have its own filesystem
    /// attributes. The function sets a mask and then sets the mask from before again.
    #[cfg(target_os = "linux")]
    fn thread_file_creation_mask() -> libc::mode_t {
        // SAFETY: `umask` only replaces the mask of the thread. It cannot fail.
        unsafe {
            let mask = libc::umask(0o022);
            libc::umask(mask);
            mask
        }
    }

    /// The test makes its temporary root under the mask that the process has. Then it binds the
    /// filesystems on a thread with the mask 0o227, which must change the mask to 0o027.
    #[cfg(target_os = "linux")]
    #[test]
    fn agent_filesystems_binding_clears_only_bit_0o200_of_the_file_mode_creation_mask() {
        let root = tempfile::tempdir().unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };

        let (bound, mask) = with_private_file_creation_mask(0o227, move || {
            let bound = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(AgentFilesystems::new(&settings));
            (bound, thread_file_creation_mask())
        });

        assert_eq!(
            mask, 0o027,
            "binding must change the mask 227 to 27, and the mask is {mask:o}"
        );
        bound.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_file_mode_creation_mask_probe_sets_0o022_and_gives_the_mask_before_the_call() {
        let (replaced, after) = with_private_file_creation_mask(0o227, || {
            (replaced_file_creation_mask(), thread_file_creation_mask())
        });

        assert_eq!(
            replaced, 0o227,
            "the call must give the mask 227 from before the call, and it gave {replaced:o}"
        );
        assert_eq!(
            after, 0o022,
            "the call must set the mask 22, and the mask is {after:o}"
        );
    }

    struct BindingSpaceObservationGuard(Option<FilesystemSpace>);

    impl Drop for BindingSpaceObservationGuard {
        fn drop(&mut self) {
            BINDING_SPACE_OBSERVATION.set(self.0);
        }
    }

    async fn with_binding_space_observation<T>(
        space: FilesystemSpace,
        binding: impl Future<Output = T>,
    ) -> T {
        let previous = BINDING_SPACE_OBSERVATION.replace(Some(space));
        let _guard = BindingSpaceObservationGuard(previous);
        binding.await
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
    async fn registry_disk_sentinel_resolves_as_unlimited() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_bytes = settings.pressure.target_available_bytes();
        let filesystems = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: observed_total_bytes,
                available_bytes: observed_total_bytes,
                total_filesystem_objects: u64::MAX,
                available_filesystem_objects: u64::MAX,
            },
            AgentFilesystems::new(&settings),
        )
        .await
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
    async fn agent_filesystems_binding_rejects_pressure_target_above_observed_capacity() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_bytes = settings.pressure.target_available_bytes() - 1;

        let result = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: observed_total_bytes,
                available_bytes: observed_total_bytes,
                total_filesystem_objects: u64::MAX,
                available_filesystem_objects: u64::MAX,
            },
            AgentFilesystems::new(&settings),
        )
        .await;

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
    async fn agent_filesystems_binding_accepts_pressure_target_equal_to_observed_capacity() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_bytes = settings.pressure.target_available_bytes();

        let result = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: observed_total_bytes,
                available_bytes: observed_total_bytes,
                total_filesystem_objects: u64::MAX,
                available_filesystem_objects: u64::MAX,
            },
            AgentFilesystems::new(&settings),
        )
        .await;

        if let Err(error) = result {
            panic!("binding rejected a pressure target equal to observed capacity: {error}");
        }
    }

    #[test]
    async fn agent_filesystems_make_the_scratch_directory_once_at_binding() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".scratch/left-by-an-earlier-process")).unwrap();
        let settings = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };

        let filesystems = AgentFilesystems::new(&settings).await.unwrap();

        assert!(
            std::fs::read_dir(root.path().join(".scratch"))
                .unwrap()
                .next()
                .is_none(),
            "binding must remove what an earlier process left in the scratch directory"
        );
        let again = HostDirectory::create_at_root(
            filesystems.provisioning(),
            std::ffi::OsStr::new(".scratch"),
        )
        .await
        .unwrap_err();
        assert_eq!(again.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    }

    #[test]
    async fn agent_filesystems_binding_rejects_object_pressure_target_above_observed_capacity() {
        let settings = FilesystemStorageConfig::default();
        let observed_total_objects = settings.pressure.target_available_filesystem_objects() - 1;

        let result = with_binding_space_observation(
            FilesystemSpace::Observed {
                total_bytes: u64::MAX,
                available_bytes: u64::MAX,
                total_filesystem_objects: observed_total_objects,
                available_filesystem_objects: observed_total_objects,
            },
            AgentFilesystems::new(&settings),
        )
        .await;

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
