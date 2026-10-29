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
use crate::host_directories::HostDirectory;
pub use crate::sandbox_filesystem::FilesystemStorageError;
use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
use crate::services::active_agents::InvocationLoops;
use crate::services::agent_filesystem_snapshots::AgentFilesystemSnapshots;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, FilesystemStorageConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::shutdown::Shutdown;
use crate::services::work_admission::WorkAdmission;
use crate::storage_volume::{
    AgentAccounting, FilesystemLimits, FilesystemSpace, FilesystemVolume, observe_space_blocking,
};
#[cfg(unix)]
pub(crate) use file_creation_mask::keep_owner_write_permission;
use golem_common::model::OwnedAgentId;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    static BINDING_SPACE_OBSERVATION: std::cell::Cell<Option<FilesystemSpace>> = const {
        std::cell::Cell::new(None)
    };
}

mod executor_directories;
#[cfg(unix)]
mod file_creation_mask;
mod lifecycle;
mod setup_error;
pub(crate) use executor_directories::{ExecutorDirectories, provision_executor_directories};
pub use setup_error::FilesystemSetupError;

#[cfg(test)]
pub(crate) use lifecycle::tests::{
    billing_metered_resident_with_open_node_for_unload_test,
    metered_resident_with_open_node_for_unload_test, no_initial_files, resident_for_unload_test,
    scratch_directory,
};
pub(crate) use lifecycle::*;

const BYTES_PER_GIB: u128 = 1024 * 1024 * 1024;

/// Resolves an agent's byte allocation into storage limits with `policy`, on storage that
/// accounts for each agent as `accounting` says.
///
/// An allocation at or above the effectively-unlimited sentinel gives `Unlimited`. A finite
/// allocation requires per-agent accounting and resolves through `policy`, which refuses zero
/// bytes. Storage without that capability rejects finite policy rather than dropping its limit.
fn resolve_storage_limits(
    policy: &FilesystemObjectLimitPolicyConfig,
    allocated_bytes: u64,
    accounting: AgentAccounting,
) -> Result<ResolvedStorageLimits, FilesystemStorageError> {
    match accounting {
        _ if allocated_bytes >= AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE => {
            Ok(ResolvedStorageLimits::Unlimited)
        }
        AgentAccounting::Unaccounted => Err(FilesystemStorageError::verification(
            "enforce finite storage limit on a quota-capable backend",
            None,
        )),
        AgentAccounting::ProjectQuotas | AgentAccounting::Development => policy
            .resolve(allocated_bytes)
            .map(ResolvedStorageLimits::Finite),
    }
}

/// Observes the volume of `provisioning` and checks that the pressure targets of `settings` fit in
/// its capacity.
fn validate_volume_capacity(
    provisioning: &SandboxFilesystemProvisioning,
    settings: &FilesystemStorageConfig,
) -> Result<(), FilesystemSetupError> {
    match observe_space_at_binding(provisioning.volume())? {
        FilesystemSpace::Observed {
            total_bytes,
            total_filesystem_objects,
            ..
        } => settings
            .pressure
            .validate_capacity(total_bytes, total_filesystem_objects)
            .map_err(Into::into),
        FilesystemSpace::Unlimited => Ok(()),
    }
}

fn observe_space_at_binding(
    volume: &FilesystemVolume,
) -> Result<FilesystemSpace, FilesystemStorageError> {
    #[cfg(test)]
    if let Some(observation) = BINDING_SPACE_OBSERVATION.get() {
        return Ok(observation);
    }

    observe_space_blocking(volume).map_err(Into::into)
}

impl FilesystemObjectLimitPolicyConfig {
    fn resolve(&self, allocated_bytes: u64) -> Result<FilesystemLimits, FilesystemStorageError> {
        if allocated_bytes == 0 {
            return Err(FilesystemStorageError::verification(
                "resolve nonzero agent filesystem storage limit",
                None,
            ));
        }

        let proportional = (u128::from(allocated_bytes) * u128::from(self.objects_per_gib()))
            .div_ceil(BYTES_PER_GIB);
        let proportional = u64::try_from(proportional).map_err(|_| {
            FilesystemStorageError::verification("derive agent filesystem object limit", None)
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
    scratch: Arc<HostDirectory>,
    file_loader: Arc<FileLoader>,
    admission: Arc<WorkAdmission>,
}

impl AgentFilesystems {
    pub(crate) fn track_shutdown(
        self: &Arc<Self>,
        shutdown: &Shutdown,
        loops: InvocationLoops,
        snapshots: Arc<AgentFilesystemSnapshots>,
    ) {
        let filesystems = Arc::clone(self);
        let token = shutdown.token();
        shutdown.spawn(async move {
            token.cancelled().await;
            filesystems
                .finish_shutdown(loops.wait_for_exit().await, &snapshots)
                .await;
        });
    }

    async fn finish_shutdown(
        &self,
        drained: Result<(), String>,
        snapshots: &AgentFilesystemSnapshots,
    ) {
        if let Err(error) = drained {
            self.retain_host_directories();
            tracing::error!(%error, "Executor work did not drain; retaining host directories");
            return;
        }
        snapshots.wait_for_shutdown().await;
        if let Err(error) = self.shut_down().await {
            tracing::error!(%error, "Failed to remove executor host directories");
        }
    }

    /// Binds sandbox storage and provisions the executor-owned `.scratch` and `.initial-files`
    /// directories. The initial-files directory goes by value to the service's one file loader.
    /// Sandbox creation and deletion do not own these directories.
    ///
    /// The service changes no state of the process. Making the host directories removes what an
    /// earlier process left under their names. Returns an error for invalid provisioning settings,
    /// host directories that cannot be made, failed volume observation, or a pressure target
    /// larger than the observed volume. After a failed space check, it discards both host
    /// directories, logs a failed discard, and returns the error of the check.
    pub(crate) async fn new(
        settings: &FilesystemStorageConfig,
        initial_files_service: Arc<InitialAgentFilesService>,
    ) -> Result<Self, FilesystemSetupError> {
        let (
            provisioning,
            ExecutorDirectories {
                scratch,
                initial_files,
            },
        ) = provision_executor_directories(&settings.mode, settings.cleanup_retry.clone()).await?;
        if let Err(error) = validate_volume_capacity(&provisioning, settings) {
            futures::future::join_all([scratch.discard(), initial_files.discard()])
                .await
                .into_iter()
                .filter_map(Result::err)
                .for_each(|cleanup| {
                    tracing::warn!(
                        error = %cleanup,
                        "Failed to discard a host directory after a failed binding"
                    );
                });
            return Err(error);
        }
        if provisioning.volume().agent_accounting() == AgentAccounting::Unaccounted {
            tracing::info!(
                "Per-agent disk limits are not enforced on storage without per-agent accounting"
            );
        }
        Ok(Self {
            provisioning,
            pressure: settings.pressure.clone(),
            filesystem_object_limit_policy: settings.filesystem_object_limit_policy.clone(),
            scratch: Arc::new(scratch),
            file_loader: Arc::new(FileLoader::new(initial_files_service, initial_files)),
            admission: Arc::default(),
        })
    }

    /// Returns the file loader that owns the `.initial-files` host directory.
    ///
    /// Each call gives the same loader. No other loader can have the directory, because the loader
    /// owns the only value of it.
    pub(crate) fn file_loader(&self) -> Arc<FileLoader> {
        Arc::clone(&self.file_loader)
    }

    /// Removes executor directories only after invocation and snapshot shutdown joined their
    /// users. Both removals run even when one fails, and failed cleanup can be retried.
    pub(crate) async fn shut_down(&self) -> Result<(), FilesystemStorageError> {
        self.admission.close();
        self.admission.wait().await;
        let (initial_files, scratch) =
            futures::join!(self.file_loader.shut_down(), self.scratch.discard());
        match (initial_files, scratch) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => {
                Err(FilesystemStorageError::from(error).during_cleanup())
            }
            (Err(initial), Err(scratch)) => {
                Err(FilesystemStorageError::from(initial).with_cleanup_failure(scratch.into()))
            }
        }
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

    /// How the storage of the agent filesystems accounts for the files of each agent.
    pub(crate) fn agent_accounting(&self) -> AgentAccounting {
        self.volume().agent_accounting()
    }

    /// Resolves an agent's byte allocation into the limits installed on a new generation.
    ///
    /// Allocations at or above the effectively-unlimited sentinel produce `Unlimited`. Smaller
    /// allocations require a quota-capable backend and derive a bounded object limit. Unsupported,
    /// zero or unrepresentable finite allocations must be rejected before creation.
    pub(crate) fn resolved_limits(
        &self,
        allocated_bytes: u64,
    ) -> Result<ResolvedStorageLimits, FilesystemStorageError> {
        resolve_storage_limits(
            &self.filesystem_object_limit_policy,
            allocated_bytes,
            self.agent_accounting(),
        )
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
        let _work = self.admission.enter().ok_or_else(|| CreateFailure {
            source: FilesystemStorageError::verification(
                "create sandbox after executor filesystem shutdown",
                None,
            ),
        })?;
        lifecycle::create_fresh_with_pressure_recovery(
            self.provisioning.clone(),
            Arc::clone(&self.scratch),
            agent,
            limits,
            pressure_recovery,
        )
        .await
    }

    pub(crate) fn retain_host_directories(&self) {
        self.scratch.retain();
        self.file_loader.retain_cache();
    }
}

/// Helpers for tests that change the file mode creation mask of one thread only.
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod file_creation_mask_for_test {
    /// Runs `f` on a new thread whose file mode creation mask is `mask`, and gives its result.
    ///
    /// The file mode creation mask is a value that all threads of a process share. So the thread
    /// first takes its own copy of the filesystem attributes of the process with
    /// `unshare(CLONE_FS)`. After that call, a change of the mask on the thread changes only the
    /// mask of the thread, and the other tests of this binary keep the mask of the process.
    pub(crate) fn with_private_file_creation_mask<T: Send + 'static>(
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
    pub(crate) fn thread_file_creation_mask() -> libc::mode_t {
        // SAFETY: `umask` only replaces the mask of the thread. It cannot fail.
        unsafe {
            let mask = libc::umask(0o022);
            libc::umask(mask);
            mask
        }
    }
}

#[cfg(test)]
mod tests;
