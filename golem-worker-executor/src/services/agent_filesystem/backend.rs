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

use super::{
    AgentFilesystemStorageLimit, AgentFilesystemUpdateEffectLease, AgentFilesystemUsage,
    FilesystemCapacity, FilesystemStorageConfig, FilesystemStorageError, OwnedAgentId,
    OwnedMutexGuard, Path, PathBuf, ResolvedAgentFilesystemLimits,
};
use crate::services::file_loader::InitialFileSource;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

pub(super) struct ProvisionedAgentFilesystem {
    backend: Arc<dyn AgentFilesystemBackend>,
    cleanup: Option<Box<dyn AgentFilesystemCleanup>>,
    lifecycle: Option<OwnedMutexGuard<()>>,
}

impl ProvisionedAgentFilesystem {
    pub fn new(
        backend: Arc<dyn AgentFilesystemBackend>,
        cleanup: Box<dyn AgentFilesystemCleanup>,
        lifecycle: OwnedMutexGuard<()>,
    ) -> Self {
        Self {
            backend,
            cleanup: Some(cleanup),
            lifecycle: Some(lifecycle),
        }
    }

    pub fn backend(&self) -> &Arc<dyn AgentFilesystemBackend> {
        &self.backend
    }

    pub async fn rollback(
        mut self,
        creation_error: FilesystemStorageError,
    ) -> FilesystemStorageError {
        let started = Instant::now();
        let result = self.delete().await;
        super::record_agent_filesystem_lifecycle("delete", result.is_ok(), started.elapsed());
        match result {
            Ok(()) => creation_error,
            Err(cleanup_error) => cleanup_error,
        }
    }

    pub async fn delete(&mut self) -> Result<(), FilesystemStorageError> {
        let Some(cleanup) = self.cleanup.as_mut() else {
            return Ok(());
        };
        let result = cleanup.delete().await;
        if result.is_ok() {
            self.cleanup.take();
            self.lifecycle.take();
        }
        result
    }
}

impl Drop for ProvisionedAgentFilesystem {
    fn drop(&mut self) {
        let Some(mut cleanup) = self.cleanup.take() else {
            return;
        };
        let lifecycle = self.lifecycle.take();
        let background = cleanup.requires_background_drop_cleanup();
        let delete = move || {
            let started = Instant::now();
            let result = cleanup.delete_blocking();
            super::record_agent_filesystem_lifecycle(
                "delete_fallback",
                result.is_ok(),
                started.elapsed(),
            );
            if let Err(error) = result {
                tracing::error!(error = %error, "Failed to delete agent runtime filesystem during fallback cleanup");
            }
            drop(lifecycle);
        };
        if background {
            std::thread::spawn(delete);
        } else {
            delete();
        }
    }
}

#[async_trait]
pub(super) trait FilesystemBackendProvisioner: Send + Sync {
    fn initial_file_cache_root(&self) -> Option<&Path>;

    async fn provision_for(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<ProvisionedAgentFilesystem, FilesystemStorageError>;

    async fn observe_capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError>;

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

pub(super) struct InitialFileMaterialization {
    pub materialization_root: PathBuf,
    pub source: InitialFileSource,
    pub target: PathBuf,
    pub read_only: bool,
    pub effect: AgentFilesystemUpdateEffectLease,
    pub staging: Option<Arc<tempfile::TempDir>>,
}

#[async_trait]
pub(super) trait AgentFilesystemBackend: Send + Sync {
    fn root(&self) -> &Path;

    fn create_staging_dir(&self) -> std::io::Result<tempfile::TempDir>;

    async fn materialize_initial_file(
        &self,
        materialization: InitialFileMaterialization,
    ) -> Result<(), FilesystemStorageError>;

    async fn observe_capacity(&self) -> Result<FilesystemCapacity, FilesystemStorageError>;

    fn quota(&self) -> Option<&dyn AgentFilesystemQuota> {
        None
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

pub(super) fn agent_filesystem_owner_path(agent_id: &OwnedAgentId) -> PathBuf {
    PathBuf::from(agent_id.environment_id.to_string())
        .join(agent_id.agent_id.component_id.to_string())
        .join(agent_id.agent_id.agent_name_encoded())
}

pub(super) struct InstalledAgentFilesystemLimit {
    pub limits: ResolvedAgentFilesystemLimits,
    pub usage: AgentFilesystemUsage,
}

#[async_trait]
pub(super) trait AgentFilesystemQuota: Send + Sync {
    async fn usage(&self) -> Result<AgentFilesystemUsage, FilesystemStorageError>;

    async fn failure_observations(
        &self,
        installed_limits: Option<ResolvedAgentFilesystemLimits>,
    ) -> Result<(AgentFilesystemUsage, Option<ResolvedAgentFilesystemLimits>), FilesystemStorageError>;

    async fn install_limit(
        &self,
        limit: AgentFilesystemStorageLimit,
        effect: AgentFilesystemUpdateEffectLease,
    ) -> Result<InstalledAgentFilesystemLimit, FilesystemStorageError>;
}

#[async_trait]
pub(super) trait AgentFilesystemCleanup: Send + Sync {
    async fn delete(&mut self) -> Result<(), FilesystemStorageError>;

    fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError>;

    fn requires_background_drop_cleanup(&self) -> bool {
        false
    }
}

pub(super) fn configured_provisioner(
    settings: &FilesystemStorageConfig,
) -> Result<Arc<dyn FilesystemBackendProvisioner>, FilesystemStorageError> {
    if settings.deterministic_root_dir.is_some() && settings.managed_xfs_root_dir.is_some() {
        return Err(FilesystemStorageError::verification(
            "select exactly one filesystem storage backend",
            Path::new("<configuration>"),
        ));
    }

    match settings.managed_xfs_root_dir.as_deref() {
        Some(root) => configured_xfs_backend(settings, root),
        None => Ok(Arc::new(super::unmanaged::UnmanagedBackend::new(
            settings.deterministic_root_dir.clone(),
            settings.cleanup_retry.clone(),
        ))),
    }
}

#[cfg(target_os = "linux")]
fn configured_xfs_backend(
    settings: &FilesystemStorageConfig,
    root: &Path,
) -> Result<Arc<dyn FilesystemBackendProvisioner>, FilesystemStorageError> {
    settings.filesystem_object_limit_policy.validate()?;
    settings.pressure.validate()?;
    let backend = super::xfs::XfsBackend::new(
        root,
        &settings.cleanup_retry,
        &settings.filesystem_object_limit_policy,
    )?;
    settings
        .pressure
        .validate_capacity(backend.observe_capacity().map_err(|error| {
            FilesystemStorageError::io("validate managed XFS pressure capacity", root, error)
        })?)?;
    Ok(Arc::new(backend))
}

#[cfg(not(target_os = "linux"))]
fn configured_xfs_backend(
    _settings: &FilesystemStorageConfig,
    root: &Path,
) -> Result<Arc<dyn FilesystemBackendProvisioner>, FilesystemStorageError> {
    Err(FilesystemStorageError::verification(
        "initialize managed XFS backend on a non-Linux platform",
        root,
    ))
}
