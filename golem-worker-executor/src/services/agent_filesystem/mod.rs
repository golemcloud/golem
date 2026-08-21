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

use crate::metrics::workers::record_agent_filesystem_lifecycle;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, FilesystemStorageConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use backend::{AgentFilesystemBackend, FilesystemBackendProvisioner};
use golem_common::model::component::InitialAgentFile;
use golem_common::model::{OwnedAgentId, RetryConfig};
use golem_common::retries::RetryState;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Instant;
use tokio::sync::{Mutex, OwnedMutexGuard};

mod backend;
mod failure;
mod initial_files;
mod mutation;
mod postcondition;
mod quota;
mod unmanaged;

#[cfg(target_os = "linux")]
mod xfs;

pub(crate) use failure::FilesystemPressureOperation;
pub(crate) use mutation::{
    AdmittedFilesystemWrite, AgentFilesystemMutationError, AgentFilesystemStreamSetupAdmission,
    AgentFilesystemUpdateEffectLease, AgentFilesystemWriteMode, AgentFilesystemWriter,
    ClassifiedFileOutputStream, FilesystemStreamMode, NativeMutationGuestError, NativeOpenOptions,
    NativeOpenResult, RequestedTime, classified_filesystem_stream_error_code,
    validate_descriptor_times, validate_directory_mutation, validate_open_capabilities,
    validate_open_flags, validate_resize, validate_two_directory_mutation,
};
use quota::FilesystemLimitExceededCallback;
pub(crate) use quota::{
    AgentFilesystemStorageLimit, AgentFilesystemUsage, FilesystemCapacity,
    ResolvedAgentFilesystemLimits,
};

#[cfg(test)]
use initial_files::InitialFileUpdateTransaction;
#[cfg(test)]
use quota::FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION;

#[cfg(test)]
mod tests;

static LIFECYCLE_LOCKS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    OnceLock::new();

#[derive(Debug)]
pub struct FilesystemStorageError {
    operation: &'static str,
    path: PathBuf,
    source: Option<std::io::Error>,
    cleanup_failed: bool,
}

impl FilesystemStorageError {
    fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: false,
        }
    }

    fn verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: false,
        }
    }

    pub(crate) fn resource_billing_transition(operation: &'static str) -> Self {
        Self::verification(operation, Path::new("<resource-meter>"))
    }

    fn cleanup_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: Some(source),
            cleanup_failed: true,
        }
    }

    fn cleanup_verification(operation: &'static str, path: &Path) -> Self {
        Self {
            operation,
            path: path.to_path_buf(),
            source: None,
            cleanup_failed: true,
        }
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.cleanup_failed
    }

    pub(crate) fn is_storage_exhaustion(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            )
        })
    }

    fn is_terminal_failure(&self) -> bool {
        self.source.as_ref().is_some_and(|source| {
            matches!(
                source.kind(),
                std::io::ErrorKind::InvalidData
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::ReadOnlyFilesystem
            ) || is_terminal_storage_errno(source)
        })
    }
}

#[cfg(target_os = "linux")]
fn is_terminal_storage_errno(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(errno) if matches!(errno, libc::EIO | libc::ESTALE | libc::ENODEV))
}

#[cfg(not(target_os = "linux"))]
fn is_terminal_storage_errno(_error: &std::io::Error) -> bool {
    false
}

impl Display for FilesystemStorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to {} agent filesystem {}",
            self.operation,
            self.path.display()
        )?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FilesystemStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone)]
pub(crate) struct AgentFilesystems {
    provisioner: Arc<dyn FilesystemBackendProvisioner>,
    pressure: FilesystemPressureConfig,
}

pub(crate) struct CreateAgentFilesystem {
    pub agent_id: OwnedAgentId,
    pub initial_files: Vec<InitialAgentFile>,
    pub file_loader: Arc<FileLoader>,
    pub resource_limits: Option<Arc<AtomicResourceEntry>>,
    pub limit_exceeded: Option<FilesystemLimitExceededCallback>,
}

impl AgentFilesystems {
    pub(crate) fn new(settings: &FilesystemStorageConfig) -> Result<Self, FilesystemStorageError> {
        Ok(Self {
            provisioner: backend::configured_provisioner(settings)?,
            pressure: settings.pressure.clone(),
        })
    }

    pub(crate) fn initial_file_cache_root(&self) -> Option<&Path> {
        self.provisioner.initial_file_cache_root()
    }

    pub(crate) async fn create_fresh(
        &self,
        request: CreateAgentFilesystem,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let mut filesystem = self.create_owned_empty(&request.agent_id).await?;
        filesystem
            .runtime
            .set_limit_exceeded_callback(request.limit_exceeded);
        if let Some(resource_limits) = request.resource_limits {
            if let Err(error) = resource_limits
                .register_agent_filesystem(request.agent_id.clone(), filesystem.runtime())
                .await
            {
                return Err(rollback_owned_filesystem(filesystem, error).await);
            }
            filesystem.limit_registration = Some(AgentFilesystemLimitRegistration {
                resource_limits,
                agent_id: request.agent_id.clone(),
                runtime: filesystem.runtime(),
            });
        }
        if let Err(error) = filesystem
            .runtime
            .replace_initial_files(
                &request.file_loader,
                request.agent_id.environment_id,
                &request.initial_files,
            )
            .await
        {
            if error.is_storage_exhaustion() {
                filesystem.runtime.notify_limit_state(true).await;
            }
            return Err(rollback_owned_filesystem(filesystem, error).await);
        }
        Ok(filesystem)
    }

    async fn create_owned_empty(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<AgentFilesystem, FilesystemStorageError> {
        let started = Instant::now();
        let result = self
            .provisioner
            .provision_for(agent_id)
            .await
            .map(|provisioned| AgentFilesystem::new(provisioned, self.pressure.clone()));
        record_agent_filesystem_lifecycle("create", result.is_ok(), started.elapsed());
        result
    }
}

pub(crate) struct AgentFilesystem {
    runtime: AgentFilesystemRuntime,
    provisioned: Option<backend::ProvisionedAgentFilesystem>,
    limit_registration: Option<AgentFilesystemLimitRegistration>,
}

struct AgentFilesystemLimitRegistration {
    resource_limits: Arc<AtomicResourceEntry>,
    agent_id: OwnedAgentId,
    runtime: AgentFilesystemRuntime,
}

impl Drop for AgentFilesystemLimitRegistration {
    fn drop(&mut self) {
        self.resource_limits
            .unregister_agent_filesystem(&self.agent_id, &self.runtime);
    }
}

impl AgentFilesystem {
    fn new(
        provisioned: backend::ProvisionedAgentFilesystem,
        pressure: FilesystemPressureConfig,
    ) -> Self {
        let runtime_backend = Arc::clone(provisioned.backend());
        Self {
            runtime: AgentFilesystemRuntime::new(runtime_backend, pressure),
            provisioned: Some(provisioned),
            limit_registration: None,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.runtime.runtime_state.backend.root()
    }

    pub(crate) fn runtime(&self) -> AgentFilesystemRuntime {
        self.runtime.clone()
    }

    pub(crate) fn seal(&self) {
        self.runtime.seal();
    }

    pub(crate) async fn close_and_delete(self) -> Result<(), FilesystemStorageError> {
        let path = self.path().to_path_buf();
        tokio::spawn(async move { self.delete_after_drain().await })
            .await
            .map_err(|error| {
                FilesystemStorageError::io(
                    "complete agent filesystem deletion",
                    &path,
                    std::io::Error::other(error),
                )
            })?
    }

    async fn delete_after_drain(mut self) -> Result<(), FilesystemStorageError> {
        let started = Instant::now();
        self.limit_registration.take();
        self.seal();
        self.runtime.drain().await;
        let result = match self.provisioned.as_mut() {
            Some(provisioned) => provisioned.delete().await,
            None => Ok(()),
        };
        record_agent_filesystem_lifecycle("delete", result.is_ok(), started.elapsed());
        result
    }
}

impl Drop for AgentFilesystem {
    fn drop(&mut self) {
        let Some(provisioned) = self.provisioned.take() else {
            return;
        };
        self.limit_registration.take();
        self.runtime.seal();
        if self.runtime.has_active_effects() {
            let runtime = self.runtime.clone();
            std::thread::spawn(move || {
                while runtime.has_active_effects() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                drop(provisioned);
            });
        } else {
            drop(provisioned);
        }
    }
}

#[derive(Clone)]
/// Cloneable handle to the synchronization and backend state shared by filesystem
/// adapters, completion tasks, quota enforcement, and usage sampling.
pub struct AgentFilesystemRuntime {
    runtime_state: Arc<AgentFilesystemRuntimeState>,
}

impl std::fmt::Debug for AgentFilesystemRuntime {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFilesystemRuntime")
            .finish_non_exhaustive()
    }
}

struct AgentFilesystemRuntimeState {
    lifecycle: AtomicUsize,
    usage_sampling: AtomicBool,
    usage_effect_epoch: std::sync::atomic::AtomicU64,
    last_effect_completion_millis: std::sync::atomic::AtomicU64,
    drained: tokio::sync::Notify,
    admission_resumed: tokio::sync::Notify,
    append: Arc<Mutex<()>>,
    namespace: Arc<Mutex<()>>,
    operations: Arc<tokio::sync::RwLock<()>>,
    backend: Arc<dyn AgentFilesystemBackend>,
    initial_files: std::sync::RwLock<HashMap<PathBuf, InitialAgentFile>>,
    pressure: FilesystemPressureConfig,
    applied_limits: std::sync::RwLock<Option<ResolvedAgentFilesystemLimits>>,
    limit_exceeded: std::sync::Mutex<Option<FilesystemLimitExceededCallback>>,
    #[allow(
        dead_code,
        reason = "runtime invalidation is exposed for filesystem host adapters"
    )]
    invalidation_notified: AtomicBool,
    #[allow(
        dead_code,
        reason = "runtime invalidation is exposed for filesystem host adapters"
    )]
    invalidated: std::sync::Mutex<Option<failure::AgentFilesystemInvalidationCallback>>,
    retry_permitted: std::sync::Mutex<Option<failure::AgentFilesystemRetryCallback>>,
    pressure_recovery: std::sync::Mutex<Option<failure::AgentFilesystemPressureRecoveryCallback>>,
    usage_observer: std::sync::Mutex<
        Option<Arc<dyn crate::services::agent_resource_billing::FilesystemUsageObserver>>,
    >,
    #[cfg(test)]
    failure_observations:
        std::sync::RwLock<Option<(Option<AgentFilesystemUsage>, FilesystemCapacity)>>,
    #[cfg(test)]
    usage_observation_fails: AtomicBool,
}

impl AgentFilesystemRuntime {
    fn new(backend: Arc<dyn AgentFilesystemBackend>, pressure: FilesystemPressureConfig) -> Self {
        Self {
            runtime_state: Arc::new(AgentFilesystemRuntimeState {
                lifecycle: AtomicUsize::new(0),
                usage_sampling: AtomicBool::new(false),
                usage_effect_epoch: std::sync::atomic::AtomicU64::new(0),
                last_effect_completion_millis: std::sync::atomic::AtomicU64::new(0),
                drained: tokio::sync::Notify::new(),
                admission_resumed: tokio::sync::Notify::new(),
                append: Arc::new(Mutex::new(())),
                namespace: Arc::new(Mutex::new(())),
                operations: Arc::new(tokio::sync::RwLock::new(())),
                backend,
                initial_files: std::sync::RwLock::new(HashMap::new()),
                pressure,
                applied_limits: std::sync::RwLock::new(None),
                limit_exceeded: std::sync::Mutex::new(None),
                invalidation_notified: AtomicBool::new(false),
                invalidated: std::sync::Mutex::new(None),
                retry_permitted: std::sync::Mutex::new(None),
                pressure_recovery: std::sync::Mutex::new(None),
                usage_observer: std::sync::Mutex::new(None),
                #[cfg(test)]
                failure_observations: std::sync::RwLock::new(None),
                #[cfg(test)]
                usage_observation_fails: AtomicBool::new(false),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new_for_test_with_observations(
            None,
            None,
            FilesystemCapacity {
                total_bytes: 1,
                available_bytes: 1,
                total_filesystem_objects: 1,
                available_filesystem_objects: 1,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn mark_read_only_for_test(&self, path: PathBuf) {
        self.runtime_state.initial_files.write().unwrap().insert(
            path,
            InitialAgentFile {
                content_hash: golem_common::model::agent::AgentFileContentHash(
                    golem_common::model::diff::Hash::empty(),
                ),
                path: golem_common::model::component::AgentFilePath::from_abs_str("/read-only")
                    .unwrap(),
                permissions: golem_common::model::component::AgentFilePermissions::ReadOnly,
                size: 0,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_observations(
        usage: Option<AgentFilesystemUsage>,
        limits: Option<ResolvedAgentFilesystemLimits>,
        capacity: FilesystemCapacity,
    ) -> Self {
        let runtime = Self::new(
            Arc::new(unmanaged::UnmanagedAgentFilesystem::new(PathBuf::from(
                "<test>",
            ))),
            FilesystemPressureConfig {
                minimum_available_bytes: 0,
                target_available_bytes: 0,
                minimum_available_filesystem_objects: 0,
                target_available_filesystem_objects: 0,
                ..FilesystemPressureConfig::default()
            },
        );
        *runtime
            .runtime_state
            .applied_limits
            .write()
            .expect("agent filesystem applied-limit lock poisoned") = limits;
        *runtime
            .runtime_state
            .failure_observations
            .write()
            .expect("agent filesystem test observation lock poisoned") = Some((usage, capacity));
        runtime
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_failed_observations() -> Self {
        let runtime = Self::new_for_test_with_capacity_observation_failure();
        runtime
            .runtime_state
            .usage_observation_fails
            .store(true, Ordering::Release);
        runtime
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_capacity_observation_failure() -> Self {
        Self::new(
            Arc::new(unmanaged::UnmanagedAgentFilesystem::new(PathBuf::from(
                "<missing-test-filesystem>",
            ))),
            FilesystemPressureConfig {
                minimum_available_bytes: 0,
                target_available_bytes: 0,
                minimum_available_filesystem_objects: 0,
                target_available_filesystem_objects: 0,
                ..FilesystemPressureConfig::default()
            },
        )
    }
}

pub(super) fn create_materialization_parent<'a>(
    root: &Path,
    target: &'a Path,
) -> std::io::Result<&'a Path> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "initial-file target has no parent",
        )
    })?;
    let relative = parent.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "initial-file target escapes the agent filesystem",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "initial-file target contains an invalid path component",
            ));
        };
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "initial-file parent is not a directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(parent)
}

pub(super) fn set_initial_file_permissions(
    file: &std::fs::File,
    read_only: bool,
) -> std::io::Result<()> {
    let mut permissions = file.metadata()?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if read_only { 0o444 } else { 0o644 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(read_only);
    file.set_permissions(permissions)
}

pub(super) async fn acquire_lifecycle_lock(path: &Path) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = LIFECYCLE_LOCKS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .expect("agent filesystem lifecycle lock registry poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(path).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    lock.lock_owned().await
}

pub(super) async fn verify_fresh_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }

    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", path, error)
    })?;
    let empty = entries
        .next_entry()
        .await
        .map_err(|error| FilesystemStorageError::io("verify empty runtime directory", path, error))?
        .is_none();
    if !empty {
        return Err(FilesystemStorageError::verification(
            "verify empty runtime directory",
            path,
        ));
    }

    Ok(())
}

pub(super) async fn verify_fresh_open_directory(path: &Path) -> Result<(), FilesystemStorageError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| FilesystemStorageError::io("verify runtime directory", path, error))?;
    if !metadata.is_dir() {
        return Err(FilesystemStorageError::verification(
            "verify fresh runtime directory",
            path,
        ));
    }

    let mut entries = tokio::fs::read_dir(path).await.map_err(|error| {
        FilesystemStorageError::io("verify empty runtime directory", path, error)
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| FilesystemStorageError::io("verify empty runtime directory", path, error))?
        .is_some()
    {
        return Err(FilesystemStorageError::verification(
            "verify empty runtime directory",
            path,
        ));
    }
    Ok(())
}

pub(super) async fn rollback_creation(
    path: &Path,
    creation_error: FilesystemStorageError,
    cleanup_retry: &RetryConfig,
) -> FilesystemStorageError {
    match remove_and_verify(path, "roll back runtime directory", cleanup_retry).await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

async fn rollback_owned_filesystem(
    filesystem: AgentFilesystem,
    creation_error: FilesystemStorageError,
) -> FilesystemStorageError {
    match filesystem.close_and_delete().await {
        Ok(()) => creation_error,
        Err(cleanup_error) => cleanup_error,
    }
}

pub(super) async fn remove_and_verify(
    path: &Path,
    operation: &'static str,
    cleanup_retry: &RetryConfig,
) -> Result<(), FilesystemStorageError> {
    let mut retry = RetryState::new(cleanup_retry);
    loop {
        retry.start_attempt();
        match remove_and_verify_once(path, operation).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !retry.failed_attempt().await {
                    return Err(error);
                }
            }
        }
    }
}

async fn remove_and_verify_once(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Ok(_) => {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| FilesystemStorageError::cleanup_io(operation, path, error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }

    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            operation, path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(operation, path, error)),
    }
}

pub(super) fn remove_and_verify_blocking(path: &Path) -> Result<(), FilesystemStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|error| {
                FilesystemStorageError::cleanup_io("delete runtime directory", path, error)
            })?;
        }
        Ok(_) => {
            std::fs::remove_file(path).map_err(|error| {
                FilesystemStorageError::cleanup_io("delete runtime directory", path, error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FilesystemStorageError::cleanup_io(
                "delete runtime directory",
                path,
                error,
            ));
        }
    }

    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(FilesystemStorageError::cleanup_verification(
            "delete runtime directory",
            path,
        )),
        Err(error) => Err(FilesystemStorageError::cleanup_io(
            "delete runtime directory",
            path,
            error,
        )),
    }
}
