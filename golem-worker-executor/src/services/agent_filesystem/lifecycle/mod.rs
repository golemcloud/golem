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

use crate::filesystem_pressure::{FilesystemWriteRecovery, FilesystemWriteRecoveryOutcome};
#[cfg(test)]
use crate::sandbox_filesystem::ScriptedSandboxFilesystem;
use crate::sandbox_filesystem::{
    FilesystemLimits, FilesystemStorageError, InstalledLimits, SandboxAccessMode,
    SandboxAttributes, SandboxDirectoryCoordinationKey, SandboxFile, SandboxFileDisposition,
    SandboxFilePermissions, SandboxFileUpdate, SandboxFilesystem, SandboxFilesystemAdapter,
    SandboxFilesystemName, SandboxFilesystemProvisioning, SandboxFollow,
    SandboxNamespaceCoordinationKey, SandboxNode, SandboxObjectKind, SandboxOpenOptions,
    SandboxOpened, SandboxPath, SandboxReadRange, SandboxResolvedNamespaceTarget,
    SandboxSymlinkTarget, SandboxSynchronization, SandboxTargetIdentity, SandboxTimeChange,
    SandboxTimeChanges, SandboxWriteAttempt, SandboxWritePlacement,
};
use crate::services::active_workers::ConcurrentAgentPermit;
use crate::services::file_loader::{FileLoader, InitialFileSource};
use crate::services::resource_usage_metering::{
    FilesystemUsage, FilesystemUsageReader, FilesystemUsageSource, MeteringOpenError,
    ResourceUsageAccount, ResourceUsageMeter, ResourceUsageMeteringWindow, create_unbound_meter,
    install_filesystem_usage, open_window, stop_metering,
};
use bytes::Bytes;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::component::{AgentFilePermissions, InitialAgentFile};
use golem_common::model::environment::EnvironmentId;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

const WRITE_PRESSURE_RECOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

mod lifecycle_stage {
    pub trait Sealed {}
}

/// A sealed marker for the valid compile-time stages of an agent filesystem.
pub(crate) trait FilesystemStage: lifecycle_stage::Sealed + 'static {}

/// A filesystem stage that owns a resource-usage meter.
pub(crate) trait MeteredStage: FilesystemStage {
    /// Returns the meter used when opening a resource-usage window for this stage.
    fn meter(&self) -> &ResourceUsageMeter;
}

pub(crate) struct Created;
pub(crate) struct Reconstructing {
    meter: ResourceUsageMeter,
    initial_files_materialized: bool,
    replay_drained: bool,
}
pub(crate) struct Resident {
    meter: ResourceUsageMeter,
}
pub(crate) struct Sealed {
    _meter: ResourceUsageMeter,
}

impl lifecycle_stage::Sealed for Created {}
impl lifecycle_stage::Sealed for Reconstructing {}
impl lifecycle_stage::Sealed for Resident {}
impl lifecycle_stage::Sealed for Sealed {}
impl FilesystemStage for Created {}
impl FilesystemStage for Reconstructing {}
impl FilesystemStage for Resident {}
impl FilesystemStage for Sealed {}
impl MeteredStage for Reconstructing {
    fn meter(&self) -> &ResourceUsageMeter {
        &self.meter
    }
}
impl MeteredStage for Resident {
    fn meter(&self) -> &ResourceUsageMeter {
        &self.meter
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedStorageLimits {
    Unlimited,
    Finite(FilesystemLimits),
}

impl ResolvedStorageLimits {
    fn sandbox(self) -> Option<FilesystemLimits> {
        match self {
            Self::Unlimited => None,
            Self::Finite(limits) => Some(limits),
        }
    }
}

pub(crate) struct PreparedInitialFiles {
    files: Vec<PreparedInitialFile>,
}

struct PreparedInitialFile {
    source: InitialFileSource,
    target: PathBuf,
    read_only: bool,
    initial_file: InitialAgentFile,
}

impl PreparedInitialFiles {
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self { files: Vec::new() }
    }
}

struct FilesystemGeneration<Adapter: SandboxFilesystemAdapter> {
    sandbox: tokio::sync::RwLock<Option<Arc<Adapter>>>,
    allocation_reader: Adapter::AllocationReader,
    registry: Arc<GenerationRegistry>,
    limits: Mutex<ResolvedStorageLimits>,
    initial_files: Mutex<HashMap<PathBuf, InitialAgentFile>>,
    pressure_recovery: Option<FilesystemWriteRecovery>,
    namespace: Arc<NamespaceCoordinator>,
}

pub(crate) struct AgentFilesystem<
    Stage: FilesystemStage,
    Adapter: SandboxFilesystemAdapter = SandboxFilesystem,
> {
    generation: Option<Arc<FilesystemGeneration<Adapter>>>,
    stage: Option<Stage>,
}

impl<Stage: FilesystemStage, Adapter: SandboxFilesystemAdapter> Debug
    for AgentFilesystem<Stage, Adapter>
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFilesystem")
            .field("stage", &std::any::type_name::<Stage>())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
type TestAgentFilesystem<Stage> = AgentFilesystem<Stage, ScriptedSandboxFilesystem>;

pub(crate) type CreatedFilesystem<Adapter = SandboxFilesystem> = AgentFilesystem<Created, Adapter>;
pub(crate) type ReconstructingFilesystem<Adapter = SandboxFilesystem> =
    AgentFilesystem<Reconstructing, Adapter>;
pub(crate) type ResidentFilesystem<Adapter = SandboxFilesystem> =
    AgentFilesystem<Resident, Adapter>;
pub(crate) type SealedFilesystem<Adapter = SandboxFilesystem> = AgentFilesystem<Sealed, Adapter>;

pub(crate) struct FilesystemGenerationHandle<Adapter: SandboxFilesystemAdapter = SandboxFilesystem>
{
    generation: Weak<FilesystemGeneration<Adapter>>,
    phase: GenerationHandlePhase,
}

#[derive(Clone, Debug)]
pub(crate) struct ResidentFilesystemActivity {
    registry: Weak<GenerationRegistry>,
}

impl<Adapter: SandboxFilesystemAdapter> Clone for FilesystemGenerationHandle<Adapter> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation.clone(),
            phase: self.phase,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationHandlePhase {
    Resident,
    Reconstruction,
}

impl<Stage: FilesystemStage, Adapter: SandboxFilesystemAdapter> Drop
    for AgentFilesystem<Stage, Adapter>
{
    fn drop(&mut self) {
        let Some(generation) = self.generation.take() else {
            return;
        };
        self.stage.take();
        generation.registry.seal();
        spawn_cleanup(generation, None);
    }
}

impl<Stage: FilesystemStage, Adapter: SandboxFilesystemAdapter> AgentFilesystem<Stage, Adapter> {
    fn into_parts(mut self) -> (Arc<FilesystemGeneration<Adapter>>, Stage) {
        let generation = self
            .generation
            .take()
            .expect("agent filesystem generation already consumed");
        let stage = self
            .stage
            .take()
            .expect("agent filesystem stage already consumed");
        (generation, stage)
    }
}

#[derive(Debug)]
pub(crate) struct CreateFailure {
    pub(crate) source: FilesystemStorageError,
}

pub(crate) struct MeteringBindFailure<Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    pub(crate) filesystem: CreatedFilesystem<Adapter>,
    pub(crate) source: Error,
}

pub(crate) struct ReconstructionFailure<Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    pub(crate) filesystem: SealedFilesystem<Adapter>,
    pub(crate) source: Error,
}

pub(crate) enum LimitTransition<Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    Resident(ResidentFilesystem<Adapter>),
    MustUnload(SealedFilesystem<Adapter>),
}

impl<Adapter: SandboxFilesystemAdapter> Debug for LimitTransition<Adapter> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resident(_) => formatter.write_str("LimitTransition::Resident"),
            Self::MustUnload(_) => formatter.write_str("LimitTransition::MustUnload"),
        }
    }
}

pub(crate) struct LimitFailure<Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    pub(crate) filesystem: SealedFilesystem<Adapter>,
    pub(crate) source: Error,
}

impl<Adapter: SandboxFilesystemAdapter> Debug for MeteringBindFailure<Adapter> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MeteringBindFailure")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl<Adapter: SandboxFilesystemAdapter> Debug for ReconstructionFailure<Adapter> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReconstructionFailure")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl<Adapter: SandboxFilesystemAdapter> Debug for LimitFailure<Adapter> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LimitFailure")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct DeleteFailure {
    pub(crate) source: FilesystemStorageError,
}

#[derive(Debug)]
pub(crate) enum Error {
    Access(AccessError),
    Sandbox(FilesystemStorageError),
    AgentQuota(FilesystemStorageError),
    PhysicalCapacity(FilesystemStorageError),
    RuntimeInvalidated,
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Access(error) => Display::fmt(error, formatter),
            Self::Sandbox(error) | Self::AgentQuota(error) | Self::PhysicalCapacity(error) => {
                Display::fmt(error, formatter)
            }
            Self::RuntimeInvalidated => formatter.write_str("agent filesystem runtime is invalid"),
        }
    }
}

impl std::error::Error for Error {}

impl From<FilesystemStorageError> for Error {
    fn from(source: FilesystemStorageError) -> Self {
        Self::Sandbox(source)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessError {
    Revoked,
    Transitioning,
    WrongGeneration,
    NotPermitted,
}

impl Display for AccessError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Revoked => formatter.write_str("agent filesystem access is revoked"),
            Self::Transitioning => formatter.write_str("agent filesystem is transitioning"),
            Self::WrongGeneration => {
                formatter.write_str("filesystem node belongs to another generation")
            }
            Self::NotPermitted => formatter.write_str("filesystem target is read-only"),
        }
    }
}

impl std::error::Error for AccessError {}

/// Validates initial-file targets and resolves their verified content sources before materialization.
///
/// Worker startup calls this before materialization. Targets are relative to the agent filesystem
/// root and must be unique; duplicate targets, inconsistent sizes for one content hash, and loader
/// failures return `Error::Sandbox` without changing the filesystem lifecycle.
pub(crate) async fn prepare_initial_files(
    file_loader: &FileLoader,
    environment_id: EnvironmentId,
    files: &[InitialAgentFile],
) -> Result<PreparedInitialFiles, Error> {
    let mut targets = HashSet::new();
    for file in files {
        let target = PathBuf::from(file.path.to_rel_string());
        if !targets.insert(target.clone()) {
            return Err(Error::Sandbox(FilesystemStorageError::verification(
                "prepare unique initial-file target",
                &target,
            )));
        }
    }

    let mut sources: HashMap<AgentFileContentHash, InitialFileSource> = HashMap::new();
    let mut prepared = Vec::with_capacity(files.len());
    for file in files {
        let target = PathBuf::from(file.path.to_rel_string());
        let source = match sources.get(&file.content_hash) {
            Some(source) if source.size() == file.size => source.clone(),
            Some(_) => {
                return Err(Error::Sandbox(FilesystemStorageError::verification(
                    "verify consistent initial-file source size",
                    &target,
                )));
            }
            None => {
                let source = file_loader
                    .get_source(environment_id, file.content_hash, file.size)
                    .await
                    .map_err(|error| {
                        Error::Sandbox(FilesystemStorageError::io(
                            "load verified initial-file source",
                            &target,
                            std::io::Error::other(error),
                        ))
                    })?;
                sources
                    .entry(file.content_hash)
                    .or_insert_with(|| source.clone())
                    .clone()
            }
        };
        prepared.push(PreparedInitialFile {
            source: source.clone(),
            target,
            read_only: file.permissions == AgentFilePermissions::ReadOnly,
            initial_file: file.clone(),
        });
    }
    Ok(PreparedInitialFiles { files: prepared })
}

#[cfg(test)]
pub(crate) fn create_fresh(
    provisioning: SandboxFilesystemProvisioning,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
) -> impl Future<Output = Result<CreatedFilesystem, CreateFailure>> + Send + 'static {
    create_fresh_with::<SandboxFilesystem>(provisioning, agent, limits)
}

/// Creates an empty sandbox filesystem and returns it in the `Created` stage.
///
/// `AgentFilesystems` calls this at worker startup with the generation's resolved limits and write
/// pressure recovery. Creation or agent-name validation failures are returned as `CreateFailure`.
pub(super) fn create_fresh_with_pressure_recovery(
    provisioning: SandboxFilesystemProvisioning,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
    pressure_recovery: FilesystemWriteRecovery,
) -> impl Future<Output = Result<CreatedFilesystem, CreateFailure>> + Send + 'static {
    create_fresh_with_recovery::<SandboxFilesystem>(
        provisioning,
        agent,
        limits,
        Some(pressure_recovery),
    )
}

#[cfg(test)]
fn create_fresh_with<Adapter: SandboxFilesystemAdapter>(
    provisioning: Adapter::Provisioning,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
) -> impl Future<Output = Result<CreatedFilesystem<Adapter>, CreateFailure>> + Send + 'static {
    create_fresh_with_recovery::<Adapter>(provisioning, agent, limits, None)
}

async fn create_fresh_with_recovery<Adapter: SandboxFilesystemAdapter>(
    provisioning: Adapter::Provisioning,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
    pressure_recovery: Option<FilesystemWriteRecovery>,
) -> Result<CreatedFilesystem<Adapter>, CreateFailure> {
    let name = SandboxFilesystemName::new(
        agent.environment_id.to_string(),
        agent.agent_id.component_id.to_string(),
        agent.agent_id.agent_name_encoded(),
    )
    .map_err(|source| CreateFailure { source })?;
    let sandbox = Adapter::create_fresh(provisioning, name, limits.sandbox())
        .await
        .map_err(|source| CreateFailure { source })?;
    let allocation_reader = sandbox.allocation_reader();
    Ok(AgentFilesystem {
        generation: Some(Arc::new(FilesystemGeneration {
            sandbox: tokio::sync::RwLock::new(Some(Arc::new(sandbox))),
            allocation_reader,
            registry: Arc::new(GenerationRegistry::new()),
            limits: Mutex::new(limits),
            initial_files: Mutex::new(HashMap::new()),
            pressure_recovery,
            namespace: Arc::new(NamespaceCoordinator::new()),
        })),
        stage: Some(Created),
    })
}

#[cfg(test)]
pub(crate) fn bind_resource_usage_metering<Adapter: SandboxFilesystemAdapter>(
    filesystem: CreatedFilesystem<Adapter>,
    account: ResourceUsageAccount,
) -> Result<ReconstructingFilesystem<Adapter>, MeteringBindFailure<Adapter>> {
    bind_configured_resource_usage_metering(
        filesystem,
        account,
        crate::services::golem_config::ResourceUsageMeteringConfig::all_enabled(),
    )
}

/// Binds configured resource metering and advances a `Created` filesystem to `Reconstructing`.
///
/// Worker startup calls this before opening a metering window or materializing initial files.
/// Filesystem usage is installed only when enabled. Failure returns the original `Created`
/// filesystem for cleanup and reports `RuntimeInvalidated` if exclusive ownership was lost.
pub(crate) fn bind_configured_resource_usage_metering<Adapter: SandboxFilesystemAdapter>(
    filesystem: CreatedFilesystem<Adapter>,
    account: ResourceUsageAccount,
    config: crate::services::golem_config::ResourceUsageMeteringConfig,
) -> Result<ReconstructingFilesystem<Adapter>, MeteringBindFailure<Adapter>> {
    let (generation, _) = filesystem.into_parts();
    let generation = match Arc::try_unwrap(generation) {
        Ok(generation) => generation,
        Err(generation) => {
            let filesystem = AgentFilesystem {
                generation: Some(generation),
                stage: Some(Created),
            };
            return Err(MeteringBindFailure {
                filesystem,
                source: Error::RuntimeInvalidated,
            });
        }
    };
    let meter = create_unbound_meter(config, account);
    let generation = Arc::new(generation);
    if config.filesystem {
        install_filesystem_usage(
            &meter,
            FilesystemUsageSource::new(Arc::new(GenerationUsageReader {
                reader: generation.allocation_reader.clone(),
            })),
        );
    }
    Ok(AgentFilesystem {
        generation: Some(generation),
        stage: Some(Reconstructing {
            meter,
            initial_files_materialized: false,
            replay_drained: false,
        }),
    })
}

/// Copies prepared initial files into a `Reconstructing` filesystem.
///
/// Callers run this once, before replay access is requested. Targets are rooted at the generation;
/// successful materialization records their permissions and enables replay access. Any seeding,
/// quota, capacity, or lifecycle failure seals the returned filesystem for cleanup.
pub(crate) fn materialize_initial_files<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
    prepared: PreparedInitialFiles,
) -> impl Future<Output = Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = complete_initial_materialization(generation, stage, prepared).await;
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        receiver
            .await
            .expect("module-owned initial-file materialization stopped unexpectedly")
    }
}

async fn complete_initial_materialization<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    mut stage: Reconstructing,
    prepared: PreparedInitialFiles,
) -> Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>> {
    if stage.initial_files_materialized || stage.replay_drained {
        return Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::RuntimeInvalidated,
        });
    }

    let mut materialized = HashMap::new();
    for file in prepared.files {
        let PreparedInitialFile {
            source,
            target,
            read_only,
            initial_file,
        } = file;
        let seed_result = async {
            let sandbox = generation
                .sandbox
                .read()
                .await
                .as_ref()
                .cloned()
                .ok_or(Error::RuntimeInvalidated)?;
            let mut budget = RetryBudget::new(2);
            loop {
                let error = match sandbox
                    .seed_file(
                        source.path(),
                        SandboxPath::at_root(target.clone()),
                        sandbox_file_permissions(read_only),
                    )
                    .await
                {
                    Ok(()) => break Ok(()),
                    Err(error) => error,
                };
                match decide_write_effect(&generation, &error, EffectEvidence::NoEffect, budget)
                    .await
                {
                    EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
                    EffectDecision::ReturnFailure(cause) => {
                        break Err(classified_error(cause, error));
                    }
                    EffectDecision::ReclaimCapacityThenRetry => {
                        break Err(Error::PhysicalCapacity(error));
                    }
                    EffectDecision::Invalidate
                    | EffectDecision::RetryAfterProvenNoEffect
                    | EffectDecision::RetryUnwrittenSuffix
                    | EffectDecision::Succeed => {
                        generation.invalidate();
                        break Err(Error::RuntimeInvalidated);
                    }
                }
            }
        }
        .await;
        if let Err(source) = seed_result {
            return Err(ReconstructionFailure {
                filesystem: sealed_filesystem(generation, stage.meter),
                source,
            });
        }
        materialized.insert(target, initial_file);
    }

    *generation.initial_files.lock().unwrap() = materialized;
    stage.initial_files_materialized = true;
    generation.registry.enable_replay_access();
    Ok(AgentFilesystem {
        generation: Some(generation),
        stage: Some(stage),
    })
}

/// Creates a generation handle for filesystem calls made while oplog replay is running.
///
/// Callers request this after initial files are materialized and before replay is drained. It
/// returns `Transitioning` outside that interval and `Revoked` after access has been sealed or
/// invalidated; handles never grant access to another filesystem generation.
pub(crate) fn reconstruction_generation_handle<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ReconstructingFilesystem<Adapter>,
) -> Result<FilesystemGenerationHandle<Adapter>, AccessError> {
    let stage = filesystem
        .stage
        .as_ref()
        .expect("agent filesystem stage already consumed");
    if !stage.initial_files_materialized || stage.replay_drained {
        return Err(AccessError::Transitioning);
    }
    let generation = filesystem
        .generation
        .as_ref()
        .expect("agent filesystem generation already consumed");
    if !generation.registry.replay_access_allowed() {
        return Err(AccessError::Revoked);
    }
    Ok(FilesystemGenerationHandle {
        generation: Arc::downgrade(generation),
        phase: GenerationHandlePhase::Reconstruction,
    })
}

/// Creates the generation handle used by a live worker for filesystem calls.
///
/// Callers use this only for a `Resident` filesystem after reconstruction succeeds. The handle is
/// generation-bound and later admissions fail once that generation is sealed or dropped.
pub(crate) fn resident_generation_handle<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ResidentFilesystem<Adapter>,
) -> FilesystemGenerationHandle<Adapter> {
    FilesystemGenerationHandle {
        generation: Arc::downgrade(
            filesystem
                .generation
                .as_ref()
                .expect("agent filesystem generation already consumed"),
        ),
        phase: GenerationHandlePhase::Resident,
    }
}

/// Returns a weak activity observer for a `Resident` filesystem generation.
///
/// Worker lifecycle code uses it to monitor terminal invalidation and in-flight effects without
/// keeping the filesystem resident. A dropped generation is reported as inactive, not failed.
pub(crate) fn filesystem_activity<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ResidentFilesystem<Adapter>,
) -> ResidentFilesystemActivity {
    ResidentFilesystemActivity {
        registry: Arc::downgrade(
            &filesystem
                .generation
                .as_ref()
                .expect("agent filesystem generation already consumed")
                .registry,
        ),
    }
}

/// Returns the configured initial-file permission for a generation-relative path.
///
/// Host adapters use this for preopen policy checks during replay or residence. Paths absent from
/// the initial-file map are read-write. A revoked or wrong-phase handle returns `AccessError`.
pub(crate) fn path_permissions<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    path: &std::path::Path,
) -> Result<AgentFilePermissions, AccessError> {
    let generation = admit(generation_handle)?;
    Ok(generation
        .initial_files
        .lock()
        .unwrap()
        .get(path)
        .map(|file| file.permissions)
        .unwrap_or(AgentFilePermissions::ReadWrite))
}

/// Starts reconciliation of a resident generation's initial files with a new component revision.
///
/// Update handling calls this through a valid generation handle. Paths are relative to the root;
/// writable files are preserved, while read-only files may be replaced or removed. Admission
/// errors are immediate, and loading or sandbox update failures are produced by the returned call.
pub(crate) fn update_initial_files(
    generation_handle: &FilesystemGenerationHandle,
    file_loader: Arc<FileLoader>,
    environment_id: EnvironmentId,
    files: Vec<InitialAgentFile>,
) -> Result<FilesystemCall<()>, Error> {
    let generation = admit(generation_handle).map_err(Error::Access)?;
    let lease = generation.registry.lease_call().map_err(Error::Access)?;
    Ok(FilesystemCall::new(lease, async move {
        complete_initial_file_update(generation, file_loader, environment_id, files).await
    }))
}

async fn complete_initial_file_update(
    generation: Arc<FilesystemGeneration<SandboxFilesystem>>,
    file_loader: Arc<FileLoader>,
    environment_id: EnvironmentId,
    files: Vec<InitialAgentFile>,
) -> Result<(), Error> {
    let sandbox = generation
        .sandbox
        .read()
        .await
        .as_ref()
        .cloned()
        .ok_or(Error::RuntimeInvalidated)?;
    let current = generation.initial_files.lock().unwrap().clone();
    let update_result = async {
        let mut desired = HashMap::new();
        for file in files {
            let relative = PathBuf::from(file.path.to_rel_string());
            if desired.insert(relative.clone(), file).is_some() {
                return Err(FilesystemStorageError::verification(
                    "materialize unique initial-file update target",
                    &relative,
                ));
            }
        }

        for (relative, file) in &desired {
            if current.get(relative).is_some_and(|existing| {
                existing.permissions == AgentFilePermissions::ReadWrite
                    && file.permissions == AgentFilePermissions::ReadOnly
            }) {
                return Err(FilesystemStorageError::verification(
                    "replace read-write initial file with read-only content",
                    relative,
                ));
            }
        }

        let preserve_candidates = desired
            .iter()
            .filter(|(relative, file)| {
                !current.contains_key(*relative)
                    && file.permissions == AgentFilePermissions::ReadWrite
            })
            .map(|(relative, _)| relative.clone())
            .collect();
        let preserved = sandbox.existing_file_targets(preserve_candidates).await?;
        let mut sources: HashMap<AgentFileContentHash, InitialFileSource> = HashMap::new();
        let mut staged = Vec::new();
        for (relative, file) in &desired {
            match current.get(relative) {
                Some(existing)
                    if existing.permissions == AgentFilePermissions::ReadWrite
                        && file.permissions == AgentFilePermissions::ReadWrite => {}
                Some(existing)
                    if existing.permissions == AgentFilePermissions::ReadOnly
                        && existing.content_hash == file.content_hash
                        && file.permissions == AgentFilePermissions::ReadOnly => {}
                None if preserved.contains(relative) => {}
                _ => {
                    let source = match sources.get(&file.content_hash) {
                        Some(source) if source.size() == file.size => source,
                        Some(_) => {
                            return Err(FilesystemStorageError::verification(
                                "verify consistent initial-file update source size",
                                relative,
                            ));
                        }
                        None => {
                            let source = file_loader
                                .get_source(environment_id, file.content_hash, file.size)
                                .await
                                .map_err(|error| {
                                    FilesystemStorageError::io(
                                        "load verified initial-file update source",
                                        relative,
                                        std::io::Error::other(error),
                                    )
                                })?;
                            sources.entry(file.content_hash).or_insert(source)
                        }
                    };
                    staged.push(SandboxFileUpdate::new(
                        relative.clone(),
                        source.path().to_path_buf(),
                        sandbox_file_permissions(
                            file.permissions == AgentFilePermissions::ReadOnly,
                        ),
                    ));
                }
            }
        }

        let removals = current
            .iter()
            .filter(|(relative, existing)| {
                existing.permissions == AgentFilePermissions::ReadOnly
                    && !desired.contains_key(*relative)
            })
            .map(|(relative, _)| relative.clone())
            .collect();
        sandbox
            .update_files(current.keys().cloned().collect(), staged, removals)
            .await?;
        *generation.initial_files.lock().unwrap() = desired;
        Ok(())
    }
    .await;

    match update_result {
        Ok(()) => Ok(()),
        Err(source) => {
            record_initial_file_update_failure(&generation.registry, &source);
            Err(Error::Sandbox(source))
        }
    }
}

fn record_initial_file_update_failure(
    registry: &GenerationRegistry,
    source: &FilesystemStorageError,
) {
    if source.cleanup_failed() || source.is_terminal_failure() {
        registry.invalidate();
    }
}

impl ResidentFilesystemActivity {
    /// Reports whether the observed resident generation has been terminally invalidated.
    ///
    /// Worker supervision polls this after residence begins. It returns `false` if the generation
    /// has been dropped, because the observer does not keep the generation alive.
    pub(crate) fn has_terminal_failure(&self) -> bool {
        self.registry
            .upgrade()
            .is_some_and(|registry| registry.is_invalidated())
    }

    /// Waits until the observed resident generation is terminally invalidated.
    ///
    /// Worker supervision uses this as an invalidation signal. The wait does not complete for a
    /// normal seal or drop; if the weak reference is already gone, it remains pending.
    pub(crate) async fn wait_for_terminal_failure(&self) {
        let Some(registry) = self.registry.upgrade() else {
            std::future::pending::<()>().await;
            return;
        };
        registry.wait_for_invalidation().await;
    }

    /// Reports whether filesystem calls are still in flight.
    ///
    /// Unload heuristics use this for a resident generation. Open node handles alone do not count
    /// as active calls, and a dropped generation returns `false`.
    pub(crate) fn has_active_effects(&self) -> bool {
        self.registry.upgrade().is_some_and(|registry| {
            let state = registry.state.lock().unwrap();
            state.calls != 0
        })
    }

    /// Returns the Unix timestamp in milliseconds of the latest completed filesystem call.
    ///
    /// Idle tracking reads this for a resident generation. It returns zero before any call
    /// completes and after the observed generation has been dropped.
    pub(crate) fn last_effect_completion_millis(&self) -> u64 {
        self.registry.upgrade().map_or(0, |registry| {
            registry
                .last_effect_completion_millis
                .load(std::sync::atomic::Ordering::Acquire)
        })
    }
}

/// Opens a resource-usage metering window for a metered filesystem stage.
///
/// Worker startup and invocation admission call this while the filesystem is `Reconstructing` or
/// `Resident`, using the concurrent-agent permit for the same account. The returned future reports
/// metering setup errors and keeps the permit associated with the window on success.
pub(crate) fn open_resource_usage_window<Stage, Adapter>(
    filesystem: &AgentFilesystem<Stage, Adapter>,
    permit: ConcurrentAgentPermit,
) -> impl Future<Output = Result<ResourceUsageMeteringWindow, MeteringOpenError>> + Send + 'static
where
    Stage: MeteredStage,
    Adapter: SandboxFilesystemAdapter,
{
    open_window(
        filesystem
            .stage
            .as_ref()
            .expect("agent filesystem stage already consumed")
            .meter(),
        permit,
    )
}

/// Ends replay access and waits for all admitted replay filesystem calls to finish.
///
/// Worker startup calls this after instance preparation and before finishing reconstruction. It
/// requires materialized initial files and an undrained `Reconstructing` stage. Lifecycle or
/// admission failure returns a sealed filesystem; success remains `Reconstructing`.
pub(crate) fn finish_replay<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
) -> impl Future<Output = Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let fenced = if !stage.initial_files_materialized || stage.replay_drained {
        Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::RuntimeInvalidated,
        })
    } else {
        match generation.registry.begin_replay_drain() {
            Ok(()) => Ok((generation, stage)),
            Err(error) => Err(ReconstructionFailure {
                filesystem: sealed_filesystem(generation, stage.meter),
                source: Error::Access(error),
            }),
        }
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = match fenced {
            Ok((generation, mut stage)) => {
                generation.registry.wait_for_calls().await;
                stage.replay_drained = true;
                Ok(AgentFilesystem {
                    generation: Some(generation),
                    stage: Some(stage),
                })
            }
            Err(failure) => Err(failure),
        };
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        let result = receiver
            .await
            .expect("module-owned replay drain stopped unexpectedly");
        if let Ok(filesystem) = &result {
            filesystem
                .generation
                .as_ref()
                .expect("reconstructing filesystem generation missing")
                .registry
                .finish_transition();
        }
        result
    }
}

/// Verifies reconstructed usage and advances the filesystem to `Resident`.
///
/// Worker startup calls this after `finish_replay`. The transition blocks new calls and waits for
/// admitted calls, then requires materialized initial files, drained replay, and usage within the
/// installed limits. Any failure returns a sealed filesystem for cleanup.
pub(crate) fn finish_reconstruction<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
) -> impl Future<Output = Result<ResidentFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let fenced = match generation.begin_transition() {
        Ok(()) => Ok((generation, stage)),
        Err(error) => Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::Access(error),
        }),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = match fenced {
            Ok((generation, stage)) => complete_reconstruction(generation, stage).await,
            Err(failure) => Err(failure),
        };
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        let result = receiver
            .await
            .expect("module-owned reconstruction transition stopped unexpectedly");
        if let Ok(filesystem) = &result {
            filesystem
                .generation
                .as_ref()
                .expect("resident filesystem generation missing")
                .registry
                .finish_transition();
        }
        result
    }
}

async fn complete_reconstruction<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    stage: Reconstructing,
) -> Result<ResidentFilesystem<Adapter>, ReconstructionFailure<Adapter>> {
    generation.registry.wait_for_calls().await;
    let result = if stage.initial_files_materialized && stage.replay_drained {
        generation
            .observe_usage()
            .await
            .and_then(|usage| verify_usage_within_limits(usage, *generation.limits.lock().unwrap()))
    } else {
        Err(Error::RuntimeInvalidated)
    };
    match result {
        Ok(()) => Ok(AgentFilesystem {
            generation: Some(generation),
            stage: Some(Resident { meter: stage.meter }),
        }),
        Err(source) => Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source,
        }),
    }
}

/// Stops reconstruction and seals the filesystem without waiting for outstanding handles.
///
/// Startup error paths use this while the filesystem is `Reconstructing`. Metering stops and new
/// calls are revoked; callers can then drain or delete the returned `Sealed` filesystem.
pub(crate) fn abort_reconstruction<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
) -> SealedFilesystem<Adapter> {
    let (generation, stage) = filesystem.into_parts();
    sealed_filesystem(generation, stage.meter)
}

/// Replaces the limits of a `Resident` filesystem generation.
///
/// Resource-limit updates call this while the worker is stopped at a lifecycle boundary. It blocks
/// new calls, waits for admitted calls, and installs sandbox byte and object limits. Success returns
/// `Resident` when usage fits or `MustUnload` with a sealed filesystem when it does not; install or
/// transition failures also return a sealed filesystem in `LimitFailure`.
pub(crate) fn set_limits<Adapter: SandboxFilesystemAdapter>(
    filesystem: ResidentFilesystem<Adapter>,
    limits: ResolvedStorageLimits,
) -> impl Future<Output = Result<LimitTransition<Adapter>, LimitFailure<Adapter>>> + Send + 'static
{
    let (generation, stage) = filesystem.into_parts();
    let fenced = match generation.begin_transition() {
        Ok(()) => Ok((generation, stage)),
        Err(error) => Err(LimitFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::Access(error),
        }),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = match fenced {
            Ok((generation, stage)) => complete_limit_transition(generation, stage, limits).await,
            Err(failure) => Err(failure),
        };
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        let result = receiver
            .await
            .expect("module-owned filesystem limit transition stopped unexpectedly");
        if let Ok(LimitTransition::Resident(filesystem)) = &result {
            filesystem
                .generation
                .as_ref()
                .expect("resident filesystem generation missing")
                .registry
                .finish_transition();
        }
        result
    }
}

async fn complete_limit_transition<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    stage: Resident,
    limits: ResolvedStorageLimits,
) -> Result<LimitTransition<Adapter>, LimitFailure<Adapter>> {
    generation.registry.wait_for_calls().await;
    let current_limits = *generation.limits.lock().unwrap();
    let sandbox_limits = match (current_limits, limits) {
        (_, ResolvedStorageLimits::Finite(limits)) => Some(limits),
        (ResolvedStorageLimits::Finite(_), ResolvedStorageLimits::Unlimited) => {
            Some(FilesystemLimits {
                allocated_bytes: u64::MAX - u16::MAX as u64,
                filesystem_objects: u64::MAX,
            })
        }
        (ResolvedStorageLimits::Unlimited, ResolvedStorageLimits::Unlimited) => None,
    };
    let result = match sandbox_limits {
        Some(limits) => generation.install_limits(limits).await.map(Some),
        None => Ok(None),
    };
    match result {
        Ok(installed) => {
            if installed
                .zip(sandbox_limits)
                .is_some_and(|(installed, requested)| installed.limits != requested)
            {
                return Err(LimitFailure {
                    filesystem: sealed_filesystem(generation, stage.meter),
                    source: Error::RuntimeInvalidated,
                });
            }
            *generation.limits.lock().unwrap() = limits;
            if installed.is_some_and(|installed| {
                decide_limit_transition(installed.allocation, installed.limits)
                    == LimitDecision::MustUnload
            }) {
                Ok(LimitTransition::MustUnload(sealed_filesystem(
                    generation,
                    stage.meter,
                )))
            } else {
                Ok(LimitTransition::Resident(AgentFilesystem {
                    generation: Some(generation),
                    stage: Some(Resident { meter: stage.meter }),
                }))
            }
        }
        Err(source) => Err(LimitFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source,
        }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LimitDecision {
    Resident,
    MustUnload,
}

fn decide_limit_transition(
    allocation: crate::sandbox_filesystem::FilesystemAllocation,
    limits: FilesystemLimits,
) -> LimitDecision {
    if allocation.allocated_bytes > limits.allocated_bytes
        || allocation.filesystem_objects > limits.filesystem_objects
    {
        LimitDecision::MustUnload
    } else {
        LimitDecision::Resident
    }
}

/// Converts a `Resident` filesystem into a `Sealed` filesystem.
///
/// Worker unload and error paths call this to stop metering and revoke new calls. Existing calls
/// and open nodes may still drain before deletion.
pub(crate) fn seal<Adapter: SandboxFilesystemAdapter>(
    filesystem: ResidentFilesystem<Adapter>,
) -> SealedFilesystem<Adapter> {
    let (generation, stage) = filesystem.into_parts();
    sealed_filesystem(generation, stage.meter)
}

/// Deletes a `Sealed` filesystem after all calls and open nodes have drained.
///
/// Cleanup paths call this after sealing or a failed transition. The returned future completes only
/// after sandbox deletion is verified; it reports deletion, ownership, or cleanup-task failures.
pub(crate) fn delete<Adapter: SandboxFilesystemAdapter>(
    filesystem: SealedFilesystem<Adapter>,
) -> impl Future<Output = Result<(), DeleteFailure>> + Send + 'static {
    let (generation, stage) = filesystem.into_parts();
    generation.registry.seal();
    drop(stage);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_cleanup(generation, Some(sender));
    async move {
        receiver.await.unwrap_or_else(|_| {
            Err(DeleteFailure {
                source: FilesystemStorageError::verification(
                    "observe agent filesystem deletion",
                    std::path::Path::new("<agent-filesystem>"),
                ),
            })
        })
    }
}

/// Deletes a filesystem that failed before metering was bound.
///
/// Startup rollback calls this with the `Created` filesystem returned by a metering bind failure.
/// It seals admission, waits for all resources to drain, and reports sandbox deletion or cleanup-task
/// failures through `DeleteFailure`.
pub(crate) fn delete_created<Adapter: SandboxFilesystemAdapter>(
    filesystem: CreatedFilesystem<Adapter>,
) -> impl Future<Output = Result<(), DeleteFailure>> + Send + 'static {
    let (generation, _) = filesystem.into_parts();
    generation.registry.seal();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_cleanup(generation, Some(sender));
    async move {
        receiver.await.unwrap_or_else(|_| {
            Err(DeleteFailure {
                source: FilesystemStorageError::verification(
                    "observe created agent filesystem deletion",
                    std::path::Path::new("<agent-filesystem>"),
                ),
            })
        })
    }
}

/// Waits until a `Sealed` filesystem has no admitted calls or open nodes.
///
/// Unload code uses this when it must retain the sandbox filesystem rather than delete it. The
/// function has no error result and does not itself release or delete sandbox resources.
pub(crate) async fn drain_sealed_filesystem<Adapter: SandboxFilesystemAdapter>(
    filesystem: &SealedFilesystem<Adapter>,
) {
    filesystem
        .generation
        .as_ref()
        .expect("sealed filesystem generation already consumed")
        .registry
        .wait_for_drain()
        .await;
}

fn sealed_filesystem<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    meter: ResourceUsageMeter,
) -> SealedFilesystem<Adapter> {
    stop_metering(&meter);
    generation.registry.seal();
    AgentFilesystem {
        generation: Some(generation),
        stage: Some(Sealed { _meter: meter }),
    }
}

fn spawn_cleanup<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    observer: Option<tokio::sync::oneshot::Sender<Result<(), DeleteFailure>>>,
) {
    spawn_module_task(async move {
        generation.registry.wait_for_drain().await;
        let sandbox = generation.sandbox.write().await.take();
        let result = match sandbox {
            Some(sandbox) => match Arc::try_unwrap(sandbox) {
                Ok(sandbox) => Adapter::delete_and_verify(sandbox)
                    .await
                    .map_err(|source| DeleteFailure { source }),
                Err(_) => Err(DeleteFailure {
                    source: FilesystemStorageError::verification(
                        "take exclusive ownership for agent filesystem deletion",
                        std::path::Path::new("<agent-filesystem>"),
                    ),
                }),
            },
            None => Ok(()),
        };
        drop(generation);
        if let Some(observer) = observer {
            let _ = observer.send(result);
        }
    });
}

fn spawn_module_task(task: impl Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
    } else {
        std::thread::Builder::new()
            .name("agent-filesystem-cleanup".to_string())
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build agent filesystem cleanup runtime")
                    .block_on(task);
            })
            .expect("failed to start agent filesystem cleanup thread");
    }
}

struct GenerationUsageReader<Reader> {
    reader: Reader,
}

impl<Reader: crate::sandbox_filesystem::SandboxFilesystemAllocationReader> FilesystemUsageReader
    for GenerationUsageReader<Reader>
{
    fn observe(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<FilesystemUsage, FilesystemStorageError>> + Send + '_>>
    {
        let reader = self.reader.clone();
        Box::pin(async move {
            match reader.read_allocation().await {
                Ok(allocation) => Ok(FilesystemUsage::Authoritative {
                    allocated_bytes: allocation.allocated_bytes,
                    filesystem_objects: allocation.filesystem_objects,
                }),
                Err(error) if error.allocation_is_unsupported() => Ok(FilesystemUsage::Unsupported),
                Err(error) => Err(error),
            }
        })
    }
}

impl<Adapter: SandboxFilesystemAdapter> FilesystemGeneration<Adapter> {
    fn begin_transition(&self) -> Result<(), AccessError> {
        self.registry.begin_transition()
    }

    async fn observe_usage(&self) -> Result<FilesystemUsage, Error> {
        self.observe_usage_sandbox().await.map_err(Error::Sandbox)
    }

    async fn observe_usage_sandbox(&self) -> Result<FilesystemUsage, FilesystemStorageError> {
        let _lease = self.registry.lease_internal_call();
        let sandbox = self.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or_else(|| {
            FilesystemStorageError::verification(
                "observe deleted agent filesystem",
                std::path::Path::new("<agent-filesystem>"),
            )
        })?;
        match sandbox.observe_allocation().await {
            Ok(allocation) => Ok(FilesystemUsage::Authoritative {
                allocated_bytes: allocation.allocated_bytes,
                filesystem_objects: allocation.filesystem_objects,
            }),
            Err(error) if error.allocation_is_unsupported() => Ok(FilesystemUsage::Unsupported),
            Err(error) => Err(error),
        }
    }

    async fn install_limits(&self, limits: FilesystemLimits) -> Result<InstalledLimits, Error> {
        let _lease = self.registry.lease_internal_call();
        let sandbox = self.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox.install_limits(limits).await.map_err(Error::Sandbox)
    }

    fn invalidate(&self) {
        self.registry.invalidate();
    }
}

fn verify_usage_within_limits(
    usage: FilesystemUsage,
    limits: ResolvedStorageLimits,
) -> Result<(), Error> {
    match (usage, limits) {
        (FilesystemUsage::Unsupported, ResolvedStorageLimits::Unlimited) => Ok(()),
        (FilesystemUsage::Unsupported, ResolvedStorageLimits::Finite(_)) => {
            Err(Error::RuntimeInvalidated)
        }
        (FilesystemUsage::Authoritative { .. }, ResolvedStorageLimits::Unlimited) => Ok(()),
        (
            FilesystemUsage::Authoritative {
                allocated_bytes,
                filesystem_objects,
            },
            ResolvedStorageLimits::Finite(limits),
        ) if allocated_bytes <= limits.allocated_bytes
            && filesystem_objects <= limits.filesystem_objects =>
        {
            Ok(())
        }
        (FilesystemUsage::Authoritative { .. }, ResolvedStorageLimits::Finite(_)) => {
            Err(Error::RuntimeInvalidated)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionState {
    Open,
    Transitioning,
    Sealed,
    Invalidated,
}

struct GenerationRegistry {
    state: Mutex<RegistryState>,
    changed: tokio::sync::Notify,
    last_effect_completion_millis: std::sync::atomic::AtomicU64,
}

struct RegistryState {
    admission: AdmissionState,
    replay_access: bool,
    calls: usize,
    nodes: usize,
}

impl GenerationRegistry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                admission: AdmissionState::Open,
                replay_access: false,
                calls: 0,
                nodes: 0,
            }),
            changed: tokio::sync::Notify::new(),
            last_effect_completion_millis: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn lease_call(self: &Arc<Self>) -> Result<CallLease, AccessError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            AdmissionState::Open => {}
            AdmissionState::Transitioning => return Err(AccessError::Transitioning),
            AdmissionState::Sealed | AdmissionState::Invalidated => {
                return Err(AccessError::Revoked);
            }
        }
        state.calls = state
            .calls
            .checked_add(1)
            .expect("filesystem call count overflowed");
        Ok(CallLease {
            registry: Arc::clone(self),
        })
    }

    fn lease_internal_call(self: &Arc<Self>) -> CallLease {
        let mut state = self.state.lock().unwrap();
        state.calls = state
            .calls
            .checked_add(1)
            .expect("filesystem call count overflowed");
        CallLease {
            registry: Arc::clone(self),
        }
    }

    fn register_node(self: &Arc<Self>) -> (NodeLease, bool) {
        let mut state = self.state.lock().unwrap();
        state.nodes = state
            .nodes
            .checked_add(1)
            .expect("filesystem node count overflowed");
        (
            NodeLease {
                registry: Arc::clone(self),
            },
            matches!(
                state.admission,
                AdmissionState::Open | AdmissionState::Transitioning
            ),
        )
    }

    fn begin_transition(&self) -> Result<(), AccessError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            AdmissionState::Open => {
                state.admission = AdmissionState::Transitioning;
                Ok(())
            }
            AdmissionState::Transitioning => Err(AccessError::Transitioning),
            AdmissionState::Sealed | AdmissionState::Invalidated => Err(AccessError::Revoked),
        }
    }

    fn enable_replay_access(&self) {
        let mut state = self.state.lock().unwrap();
        if state.admission == AdmissionState::Open {
            state.replay_access = true;
        }
    }

    fn replay_access_allowed(&self) -> bool {
        self.state.lock().unwrap().replay_access
    }

    fn begin_replay_drain(&self) -> Result<(), AccessError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            AdmissionState::Open if state.replay_access => {
                state.replay_access = false;
                state.admission = AdmissionState::Transitioning;
                Ok(())
            }
            AdmissionState::Open | AdmissionState::Transitioning => Err(AccessError::Transitioning),
            AdmissionState::Sealed | AdmissionState::Invalidated => Err(AccessError::Revoked),
        }
    }

    fn finish_transition(&self) {
        let mut state = self.state.lock().unwrap();
        if state.admission == AdmissionState::Transitioning {
            state.admission = AdmissionState::Open;
        }
        self.changed.notify_waiters();
    }

    fn seal(&self) {
        let mut state = self.state.lock().unwrap();
        state.replay_access = false;
        if state.admission != AdmissionState::Invalidated {
            state.admission = AdmissionState::Sealed;
        }
        self.changed.notify_waiters();
    }

    fn invalidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.replay_access = false;
        state.admission = AdmissionState::Invalidated;
        drop(state);
        self.changed.notify_waiters();
    }

    fn is_invalidated(&self) -> bool {
        self.state.lock().unwrap().admission == AdmissionState::Invalidated
    }

    async fn wait_for_invalidation(&self) {
        loop {
            let notified = self.changed.notified();
            if self.is_invalidated() {
                return;
            }
            notified.await;
        }
    }

    async fn wait_for_calls(&self) {
        loop {
            let notified = self.changed.notified();
            if self.state.lock().unwrap().calls == 0 {
                return;
            }
            notified.await;
        }
    }

    async fn wait_for_drain(&self) {
        loop {
            let notified = self.changed.notified();
            let drained = {
                let state = self.state.lock().unwrap();
                state.calls == 0 && state.nodes == 0
            };
            if drained {
                return;
            }
            notified.await;
        }
    }
}

struct CallLease {
    registry: Arc<GenerationRegistry>,
}

impl Drop for CallLease {
    fn drop(&mut self) {
        let mut state = self.registry.state.lock().unwrap();
        debug_assert!(state.calls != 0);
        state.calls -= 1;
        drop(state);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        self.registry
            .last_effect_completion_millis
            .store(now, std::sync::atomic::Ordering::Release);
        self.registry.changed.notify_waiters();
    }
}

struct NodeLease {
    registry: Arc<GenerationRegistry>,
}

impl Drop for NodeLease {
    fn drop(&mut self) {
        let mut state = self.registry.state.lock().unwrap();
        debug_assert!(state.nodes != 0);
        state.nodes -= 1;
        drop(state);
        self.registry.changed.notify_waiters();
    }
}

struct NamespaceCoordinator {
    state: Mutex<NamespaceCoordinatorState>,
    changed: tokio::sync::Notify,
}

struct NamespaceCoordinatorState {
    next_id: u64,
    active: Vec<ActiveNamespaceEdit>,
}

struct ActiveNamespaceEdit {
    id: u64,
    kind: NamespaceCoordinationKind,
    keys: NamespaceCoordinationKeys,
    directories_installed: bool,
}

#[derive(Clone, Default)]
struct NamespaceCoordinationKeys {
    namespace: Vec<SandboxNamespaceCoordinationKey>,
    directories: Vec<SandboxDirectoryCoordinationKey>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NamespaceCoordinationKind {
    Observe,
    Edit,
}

impl NamespaceCoordinator {
    fn new() -> Self {
        Self {
            state: Mutex::new(NamespaceCoordinatorState {
                next_id: 0,
                active: Vec::new(),
            }),
            changed: tokio::sync::Notify::new(),
        }
    }

    async fn coordinate(
        self: &Arc<Self>,
        kind: NamespaceCoordinationKind,
        namespace: Vec<SandboxNamespaceCoordinationKey>,
    ) -> NamespaceCoordination {
        let namespace = deduplicate(namespace);
        loop {
            let notified = self.changed.notified();
            let admitted = {
                let mut state = self.state.lock().unwrap();
                let keys = NamespaceCoordinationKeys {
                    namespace: namespace.clone(),
                    directories: Vec::new(),
                };
                if state
                    .active
                    .iter()
                    .any(|active| coordination_conflicts(kind, &keys, active))
                {
                    None
                } else {
                    let id = state.next_id;
                    state.next_id = state.next_id.wrapping_add(1);
                    state.active.push(ActiveNamespaceEdit {
                        id,
                        kind,
                        keys,
                        directories_installed: false,
                    });
                    Some(id)
                }
            };
            if let Some(id) = admitted {
                return NamespaceCoordination {
                    coordinator: Arc::clone(self),
                    id,
                    active: true,
                };
            }
            notified.await;
        }
    }
}

struct NamespaceCoordination {
    coordinator: Arc<NamespaceCoordinator>,
    id: u64,
    active: bool,
}

impl NamespaceCoordination {
    fn extend(&mut self, directories: Vec<SandboxDirectoryCoordinationKey>) -> bool {
        let directories = deduplicate(directories);
        let mut state = self.coordinator.state.lock().unwrap();
        let index = state
            .active
            .iter()
            .position(|active| active.id == self.id)
            .expect("active namespace coordination lease missing");
        let mut extended = state.active[index].keys.clone();
        extended.directories = directories;
        let kind = state.active[index].kind;
        let blocked = state.active.iter().any(|active| {
            active.id != self.id
                && active.directories_installed
                && coordination_conflicts(kind, &extended, active)
        });
        let result = if blocked {
            state.active.swap_remove(index);
            self.active = false;
            false
        } else {
            state.active[index].keys = extended;
            state.active[index].directories_installed = true;
            true
        };
        drop(state);
        self.coordinator.changed.notify_waiters();
        result
    }
}

impl Drop for NamespaceCoordination {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.coordinator.state.lock().unwrap();
        let index = state
            .active
            .iter()
            .position(|active| active.id == self.id)
            .expect("active namespace coordination lease missing");
        state.active.swap_remove(index);
        drop(state);
        self.coordinator.changed.notify_waiters();
    }
}

fn coordination_conflicts(
    kind: NamespaceCoordinationKind,
    keys: &NamespaceCoordinationKeys,
    active: &ActiveNamespaceEdit,
) -> bool {
    if kind == NamespaceCoordinationKind::Observe
        && active.kind == NamespaceCoordinationKind::Observe
    {
        return false;
    }
    keys.namespace.iter().any(|left| {
        active
            .keys
            .namespace
            .iter()
            .any(|right| left.may_conflict_with(right))
            || active.keys.directories.iter().any(|right| left == right)
    }) || keys.directories.iter().any(|left| {
        active.keys.namespace.iter().any(|right| left == right)
            || active.keys.directories.iter().any(|right| left == right)
    })
}

fn deduplicate<T: Eq>(values: Vec<T>) -> Vec<T> {
    values.into_iter().fold(Vec::new(), |mut unique, value| {
        if !unique.contains(&value) {
            unique.push(value);
        }
        unique
    })
}

type CallTask<T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'static>>;

#[must_use]
pub(crate) struct FilesystemCall<T: 'static> {
    state: CallState<T>,
}

impl<T: 'static> Debug for FilesystemCall<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemCall")
            .finish_non_exhaustive()
    }
}

enum CallState<T: 'static> {
    Unstarted {
        task: Option<CallTask<T>>,
        lease: Option<CallLease>,
    },
    CallerDriven {
        task: CallTask<T>,
        lease: Option<CallLease>,
    },
    Done,
}

impl<T: Send + 'static> FilesystemCall<T> {
    fn new(
        lease: CallLease,
        task: impl Future<Output = Result<T, Error>> + Send + 'static,
    ) -> Self {
        Self {
            state: CallState::Unstarted {
                task: Some(Box::pin(task)),
                lease: Some(lease),
            },
        }
    }
}

impl<T: Send + 'static> Future for FilesystemCall<T> {
    type Output = Result<T, Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match &mut self.state {
                CallState::Unstarted { task, lease } => {
                    let task = task.take().expect("filesystem call task already started");
                    let lease = lease.take().expect("filesystem call lease already started");
                    self.state = CallState::CallerDriven {
                        task,
                        lease: Some(lease),
                    };
                }
                CallState::CallerDriven { task, lease } => {
                    let result = match task.as_mut().poll(context) {
                        Poll::Ready(result) => result,
                        Poll::Pending => return Poll::Pending,
                    };
                    drop(lease.take());
                    self.state = CallState::Done;
                    return Poll::Ready(result);
                }
                CallState::Done => panic!("filesystem call polled after completion"),
            }
        }
    }
}

impl<T: 'static> Drop for FilesystemCall<T> {
    fn drop(&mut self) {
        if !matches!(self.state, CallState::CallerDriven { .. }) {
            return;
        }
        let CallState::CallerDriven { task, mut lease } =
            std::mem::replace(&mut self.state, CallState::Done)
        else {
            unreachable!()
        };
        spawn_module_task(async move {
            drop(task.await);
            drop(lease.take());
        });
    }
}

#[must_use]
pub(crate) struct FilesystemRelease {
    state: ReleaseState,
}

enum ReleaseState {
    Unstarted { task: Option<CallTask<()>> },
    CallerDriven { task: Option<CallTask<()>> },
    Done,
}

impl Future for FilesystemRelease {
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match &mut self.state {
                ReleaseState::Unstarted { task } => {
                    let task = task
                        .take()
                        .expect("filesystem release task already started");
                    self.state = ReleaseState::CallerDriven { task: Some(task) };
                }
                ReleaseState::CallerDriven { task } => {
                    let result = match task
                        .as_mut()
                        .expect("filesystem release task missing")
                        .as_mut()
                        .poll(context)
                    {
                        Poll::Ready(result) => result,
                        Poll::Pending => return Poll::Pending,
                    };
                    task.take();
                    self.state = ReleaseState::Done;
                    return Poll::Ready(result);
                }
                ReleaseState::Done => panic!("filesystem release polled after completion"),
            }
        }
    }
}

impl Drop for FilesystemRelease {
    fn drop(&mut self) {
        let task = match &mut self.state {
            ReleaseState::Unstarted { task } | ReleaseState::CallerDriven { task } => task.take(),
            ReleaseState::Done => return,
        };
        if let Some(task) = task {
            spawn_module_task(async move {
                drop(task.await);
            });
        }
    }
}

pub(crate) enum OpenNode {
    File(File),
    Directory(Directory),
}

impl OpenNode {
    /// Returns whether this open node is a file or directory.
    ///
    /// Host adapters use this after `open`; the value describes the opened object in its original
    /// generation and performs no new lifecycle admission.
    pub(crate) fn kind(&self) -> ObjectKind {
        match self {
            Self::File(_) => ObjectKind::File,
            Self::Directory(_) => ObjectKind::Directory,
        }
    }

    /// Returns the access mode granted when this node was opened.
    ///
    /// Host adapters use it to enforce descriptor capabilities before submitting mutations. The
    /// mode remains available until release and does not revalidate generation access.
    pub(crate) fn access(&self) -> AccessMode {
        node_ownership(self).access
    }
}

pub(crate) struct File {
    ownership: NodeOwnership,
}

pub(crate) struct Directory {
    ownership: NodeOwnership,
}

struct NodeOwnership {
    sandbox: Option<SandboxNode>,
    generation_id: usize,
    access: AccessMode,
    node_lease: Option<NodeLease>,
    release: Option<Box<dyn FnOnce(SandboxNode, NodeLease) -> FilesystemRelease + Send>>,
}

impl Debug for File {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("File").finish_non_exhaustive()
    }
}

impl Debug for Directory {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Directory").finish_non_exhaustive()
    }
}

impl Drop for NodeOwnership {
    fn drop(&mut self) {
        let (Some(sandbox), Some(node_lease), Some(release)) = (
            self.sandbox.take(),
            self.node_lease.take(),
            self.release.take(),
        ) else {
            return;
        };
        drop(release(sandbox, node_lease));
    }
}

impl NodeOwnership {
    fn sandbox(&self) -> &SandboxNode {
        self.sandbox
            .as_ref()
            .expect("filesystem node already released")
    }

    fn into_release(mut self) -> FilesystemRelease {
        let sandbox = self
            .sandbox
            .take()
            .expect("filesystem node already released");
        let node_lease = self
            .node_lease
            .take()
            .expect("filesystem node lease missing");
        self.release
            .take()
            .expect("filesystem node release missing")(sandbox, node_lease)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PathTarget {
    sandbox: SandboxPath,
    generation_id: usize,
    access: AccessMode,
}

impl PathTarget {
    /// Creates a read-write path target relative to the filesystem root.
    ///
    /// Callers use this for preopens and root-relative host paths with an admitted reconstruction
    /// or resident handle. The target is bound to that generation; revoked handles fail here, and
    /// later operations reject use with another generation.
    pub(crate) fn at_root<Adapter: SandboxFilesystemAdapter>(
        generation_handle: &FilesystemGenerationHandle<Adapter>,
        path: impl Into<PathBuf>,
    ) -> Result<Self, AccessError> {
        let generation = admit(generation_handle)?;
        let path = path.into();
        Ok(Self {
            sandbox: SandboxPath::at_root(path),
            generation_id: Arc::as_ptr(&generation) as usize,
            access: AccessMode::ReadWrite,
        })
    }

    /// Creates a path target relative to an open directory.
    ///
    /// Host adapters use this for descriptor-relative paths. The target inherits the directory's
    /// generation and access mode; operations that enforce path writability use the inherited mode.
    pub(crate) fn at(directory: &Directory, path: impl Into<PathBuf>) -> Self {
        let generation_id = directory.ownership.generation_id;
        let access = directory.ownership.access;
        let path = path.into();
        let SandboxNode::Directory(directory) = directory.ownership.sandbox() else {
            unreachable!("directory wrapper must contain a sandbox directory")
        };
        Self {
            sandbox: SandboxPath::at(directory.clone(), path),
            generation_id,
            access,
        }
    }
}

pub(crate) enum Target<'a> {
    Open(&'a OpenNode),
    Path(&'a PathTarget, Follow),
}

#[derive(Clone)]
enum AttributeTarget {
    Open(SandboxNode),
    Path {
        target: SandboxPath,
        follow: SandboxFollow,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WritePlacement {
    At(u64),
    Append,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Synchronization {
    Data,
    DataAndMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Follow {
    Yes,
    No,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObjectKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessMode {
    Read,
    Write,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenOptions {
    Existing {
        expected: ObjectKind,
        access: AccessMode,
        follow: Follow,
    },
    File {
        access: AccessMode,
        disposition: FileDisposition,
        follow: Follow,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileDisposition {
    CreateIfMissing,
    CreateExclusive,
    TruncateExisting,
    CreateOrTruncate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimeChange {
    Keep,
    Now,
    Set(std::time::SystemTime),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TimeChanges {
    pub(crate) accessed: TimeChange,
    pub(crate) modified: TimeChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttributeChanges {
    Times(TimeChanges),
    File { size: u64, times: TimeChanges },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NewObject {
    Directory,
    Symlink(SymlinkTarget),
}

#[derive(Clone, Debug)]
pub(crate) enum NamespaceEdit {
    Insert {
        destination: PathTarget,
        object: NewObject,
    },
    Link {
        source: PathTarget,
        destination: PathTarget,
    },
    Move {
        source: PathTarget,
        destination: PathTarget,
    },
    Remove {
        target: PathTarget,
        expected: ObjectKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReadRange {
    pub(crate) offset: u64,
    pub(crate) length: usize,
}

pub(crate) type ReadResult = Bytes;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attributes {
    pub(crate) kind: ObjectKind,
    pub(crate) link_count: u64,
    pub(crate) size: u64,
    pub(crate) accessed: Option<std::time::SystemTime>,
    pub(crate) modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryEntry {
    pub(crate) name: std::ffi::OsString,
    pub(crate) kind: ObjectKind,
}

pub(crate) type DirectoryEntries = Vec<DirectoryEntry>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymlinkTarget(pub(crate) PathBuf);

pub(crate) struct Opened {
    pub(crate) node: OpenNode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WriteResult {
    pub(crate) written: u64,
}

/// Opens the target according to the expected kind, access mode, symlink policy, and disposition.
///
/// Requires an admitted reconstruction or resident handle and a target from the same generation.
/// Invalid lifecycle or generation state returns `AccessError` immediately. Native, permission,
/// quota, capacity, or invalidation errors are produced by the returned call; success returns an
/// open node whose lifetime delays generation cleanup.
pub(crate) fn open<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: PathTarget,
    options: OpenOptions,
) -> Result<FilesystemCall<Opened>, AccessError> {
    let generation = admit(generation_handle)?;
    validate_path_generation(&generation, &target)?;
    let opened_access = open_access(options);
    let lease = generation.registry.lease_call()?;
    Ok(FilesystemCall::new(lease, async move {
        execute_coordinated_open(generation, target.sandbox, options, opened_access).await
    }))
}

/// Reads up to the requested byte range from an open file.
///
/// Host adapters call this with a file from the same admitted reconstruction or resident
/// generation. Revoked, transitioning, or cross-generation access fails immediately; sandbox read
/// and terminal-generation failures are returned when the deferred call is awaited.
pub(crate) fn read_file<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    file: &File,
    range: ReadRange,
) -> Result<FilesystemCall<ReadResult>, AccessError> {
    let generation = admit_node(generation_handle, file.ownership.generation_id)?;
    let lease = generation.registry.lease_call()?;
    let SandboxNode::File(file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let file = file.clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox
            .read(
                &file,
                SandboxReadRange {
                    offset: range.offset,
                    length: range.length,
                },
            )
            .await
            .map_err(|source| classify_query_error(&generation, source))
    }))
}

/// Reads attributes from an open node or from a path target.
///
/// For path targets, `Follow` controls whether the final symlink is dereferenced; open targets
/// address the opened object directly. The target must belong to the admitted reconstruction or
/// resident generation. Admission errors are immediate, while sandbox query errors are deferred.
pub(crate) fn attributes<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: Target<'_>,
) -> Result<FilesystemCall<Attributes>, AccessError> {
    let generation = admit(generation_handle)?;
    let target = sandbox_target(&generation, target)?;
    let lease = generation.registry.lease_call()?;
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        match read_sandbox_attributes(sandbox.as_ref(), target).await {
            Ok(attributes) => Ok(agent_attributes(attributes)),
            Err(source) => Err(classify_query_error(&generation, source)),
        }
    }))
}

/// Tests whether two open nodes refer to the same sandbox filesystem object.
///
/// Host adapters use this for descriptor identity checks. Both nodes must belong to the admitted
/// generation or `WrongGeneration` is returned immediately; sandbox comparison failures are
/// returned by the deferred call.
pub(crate) fn is_same_object<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    left: &OpenNode,
    right: &OpenNode,
) -> Result<FilesystemCall<bool>, AccessError> {
    let left_ownership = node_ownership(left);
    let generation = admit_node(generation_handle, left_ownership.generation_id)?;
    let right_ownership = node_ownership(right);
    if right_ownership.generation_id != left_ownership.generation_id {
        return Err(AccessError::WrongGeneration);
    }
    let lease = generation.registry.lease_call()?;
    let left = left_ownership.sandbox().clone();
    let right = right_ownership.sandbox().clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox
            .is_same_open_object(left, right)
            .await
            .map_err(|source| classify_query_error(&generation, source))
    }))
}

/// Lists the immediate entries of an open directory.
///
/// The directory must belong to the admitted reconstruction or resident generation. Lifecycle and
/// generation errors are returned immediately; directory-read failures or terminal invalidation
/// are returned by the deferred call.
pub(crate) fn list_directory<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    directory: &Directory,
) -> Result<FilesystemCall<DirectoryEntries>, AccessError> {
    let generation = admit_node(generation_handle, directory.ownership.generation_id)?;
    let lease = generation.registry.lease_call()?;
    let SandboxNode::Directory(directory) = directory.ownership.sandbox() else {
        unreachable!("directory wrapper must contain a sandbox directory")
    };
    let directory = directory.clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        match sandbox.read_directory(&directory).await {
            Ok(entries) => Ok(entries
                .into_iter()
                .map(|entry| DirectoryEntry {
                    name: entry.name,
                    kind: agent_object_kind(entry.kind),
                })
                .collect()),
            Err(source) => Err(classify_query_error(&generation, source)),
        }
    }))
}

/// Reads the stored target of a symlink at a path without following the final link.
///
/// The path must belong to the admitted reconstruction or resident generation. Admission and
/// generation errors are immediate; missing paths, wrong object kinds, and sandbox failures are
/// returned by the deferred call.
pub(crate) fn symlink_target<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: PathTarget,
) -> Result<FilesystemCall<SymlinkTarget>, AccessError> {
    let generation = admit(generation_handle)?;
    validate_path_generation(&generation, &target)?;
    let lease = generation.registry.lease_call()?;
    let path = target.sandbox.clone();
    Ok(FilesystemCall::new(lease, async move {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        match sandbox.read_link(path).await {
            Ok(target) => Ok(SymlinkTarget(target.0)),
            Err(source) => Err(classify_query_error(&generation, source)),
        }
    }))
}

/// Writes bytes to an open file at an offset or at the coordinated append position.
///
/// The file must belong to the admitted reconstruction or resident generation. Admission failures
/// are immediate. The deferred result distinguishes quota and physical-capacity failures, may
/// report a successfully written prefix, and invalidates the generation when the effect is unsafe
/// to determine.
pub(crate) fn write<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    file: &File,
    placement: WritePlacement,
    bytes: Bytes,
) -> Result<FilesystemCall<WriteResult>, AccessError> {
    let generation = admit_node(generation_handle, file.ownership.generation_id)?;
    let lease = generation.registry.lease_call()?;
    let SandboxNode::File(file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let file = file.clone();
    Ok(FilesystemCall::new(lease, async move {
        execute_write(generation, file, placement, bytes).await
    }))
}

/// Applies timestamp changes, or file size plus timestamp changes, to a target.
///
/// Open targets address the opened object; path targets obey their final-symlink `Follow` setting.
/// The target must be writable and belong to the admitted generation, and size changes require an
/// open file. These checks return `AccessError` immediately; sandbox and capacity errors are deferred.
pub(crate) fn set_attributes<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: Target<'_>,
    changes: AttributeChanges,
) -> Result<FilesystemCall<()>, AccessError> {
    let generation = admit(generation_handle)?;
    authorize_attribute_target(&target, changes)?;
    let coordinated_path = match &target {
        Target::Path(target, follow) => Some((target.sandbox.clone(), *follow)),
        Target::Open(_) => None,
    };
    let target = sandbox_target(&generation, target)?;
    let lease = generation.registry.lease_call()?;
    Ok(FilesystemCall::new(lease, async move {
        match coordinated_path {
            Some((target, follow)) => {
                execute_coordinated_path_attribute_changes(generation, target, follow, changes)
                    .await
            }
            None => execute_attribute_changes(generation, target, changes).await,
        }
    }))
}

/// Restores recorded timestamps on an open or path target during durable replay.
///
/// Replay adapters call this with a reconstruction handle after replaying the matching metadata
/// operation. Path targets obey `Follow`; `Keep` leaves a timestamp unchanged. Generation and
/// admission errors are immediate, while restoration or invalidation errors are deferred.
pub(crate) fn restore_times<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: Target<'_>,
    times: TimeChanges,
) -> Result<FilesystemCall<()>, AccessError> {
    let generation = admit(generation_handle)?;
    let target = sandbox_target(&generation, target)?;
    let lease = generation.registry.lease_call()?;
    Ok(FilesystemCall::new(lease, async move {
        execute_set_times(generation, target, times).await
    }))
}

/// Applies one generation-local namespace edit.
///
/// Targets are interpreted relative to their root or open directory. Every source and destination
/// must belong to the admitted generation and permit writes. Invalid lifecycle, generation, or
/// access state fails immediately; sandbox, quota, capacity, and uncertain-effect failures are
/// returned by the coordinated deferred call.
pub(crate) fn edit_namespace<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    edit: NamespaceEdit,
) -> Result<FilesystemCall<()>, AccessError> {
    let generation = admit(generation_handle)?;
    validate_namespace_generation(&generation, &edit)?;
    authorize_namespace_edit(&edit)?;
    let lease = generation.registry.lease_call()?;
    Ok(FilesystemCall::new(lease, async move {
        execute_namespace_edit(generation, edit).await
    }))
}

/// Synchronizes an open node's data, or its data and metadata, with sandbox storage.
///
/// Host adapters call this for an open node in the admitted reconstruction or resident generation.
/// Admission errors are immediate. Any sandbox synchronization failure invalidates the generation
/// and is returned as `RuntimeInvalidated` by the deferred call.
pub(crate) fn synchronize<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    node: &OpenNode,
    level: Synchronization,
) -> Result<FilesystemCall<()>, AccessError> {
    let ownership = node_ownership(node);
    let generation = admit_node(generation_handle, ownership.generation_id)?;
    let lease = generation.registry.lease_call()?;
    let node = ownership.sandbox().clone();
    Ok(FilesystemCall::new(lease, async move {
        execute_synchronize(generation, node, level).await
    }))
}

/// Explicitly releases an open file or directory.
///
/// Host adapters await this when closing a descriptor; dropping an `OpenNode` starts the same work
/// in the background. The returned release completes the sandbox close and relinquishes the node
/// lease even if the caller drops it, and reports sandbox or generation invalidation errors.
pub(crate) fn release(node: OpenNode) -> FilesystemRelease {
    match node {
        OpenNode::File(file) => file.ownership.into_release(),
        OpenNode::Directory(directory) => directory.ownership.into_release(),
    }
}

impl<Adapter: SandboxFilesystemAdapter> FilesystemGeneration<Adapter> {
    async fn register_opened(
        self: Arc<Self>,
        opened: SandboxOpened,
        access: AccessMode,
    ) -> Result<Opened, Error> {
        let node = opened.into_node();
        let (node_lease, return_to_caller) = self.registry.register_node();
        let open_node = self.wrap_node(node, access, node_lease);
        if return_to_caller {
            Ok(Opened { node: open_node })
        } else {
            release(open_node).await?;
            Err(Error::Access(AccessError::Revoked))
        }
    }

    fn wrap_node(
        self: &Arc<Self>,
        node: SandboxNode,
        access: AccessMode,
        node_lease: NodeLease,
    ) -> OpenNode {
        let generation_id = Arc::as_ptr(self) as usize;
        let generation = Arc::clone(self);
        let ownership = NodeOwnership {
            sandbox: Some(node.clone()),
            generation_id,
            access,
            node_lease: Some(node_lease),
            release: Some(Box::new(move |node, node_lease| {
                generation.start_release(node, node_lease)
            })),
        };
        match node {
            SandboxNode::File(_) => OpenNode::File(File { ownership }),
            SandboxNode::Directory(_) => OpenNode::Directory(Directory { ownership }),
        }
    }

    fn start_release(
        self: Arc<Self>,
        node: SandboxNode,
        node_lease: NodeLease,
    ) -> FilesystemRelease {
        let call_lease = self.registry.lease_internal_call();
        FilesystemRelease {
            state: ReleaseState::Unstarted {
                task: Some(Box::pin(async move {
                    let result = execute_release(Arc::clone(&self), node).await;
                    drop(node_lease);
                    drop(call_lease);
                    result
                })),
            },
        }
    }
}

async fn execute_coordinated_open<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    options: OpenOptions,
    opened_access: AccessMode,
) -> Result<Opened, Error> {
    let Some(kind) = open_namespace_coordination(options) else {
        let opened = execute_open(Arc::clone(&generation), target, options).await?;
        return generation.register_opened(opened, opened_access).await;
    };
    loop {
        let resolved = resolve_namespace_target(&generation, target.clone()).await?;
        let mut coordination = generation
            .namespace
            .coordinate(kind, vec![resolved.coordination_key()])
            .await;
        authorize_resolved_open(&generation, &resolved, options).await?;
        if open_returns_directory(options) {
            let opened = execute_open(Arc::clone(&generation), resolved.target(), options).await?;
            let directory_key = opened
                .directory_coordination_key()
                .expect("sandbox directory open must carry a coordination identity");
            if !coordination.extend(vec![directory_key.clone()]) {
                drop(opened);
                continue;
            }
            return generation.register_opened(opened, opened_access).await;
        }
        if !coordination.extend(Vec::new()) {
            continue;
        }
        let opened = execute_open(Arc::clone(&generation), resolved.target(), options).await?;
        return generation.register_opened(opened, opened_access).await;
    }
}

async fn resolve_namespace_target<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
) -> Result<SandboxResolvedNamespaceTarget, Error> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
    sandbox
        .resolve_namespace_target(target)
        .await
        .map_err(|source| classify_query_error(generation, source))
}

async fn authorize_resolved_open<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxResolvedNamespaceTarget,
    options: OpenOptions,
) -> Result<(), Error> {
    if !open_requires_mutable_target(options) {
        return Ok(());
    }
    let follow = match options {
        OpenOptions::Existing { follow, .. } | OpenOptions::File { follow, .. } => follow,
    };
    let target = resolved_policy_target(generation, target, sandbox_follow(follow))?;
    authorize_policy_target(generation, &target).await
}

fn resolved_policy_target<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxResolvedNamespaceTarget,
    follow: SandboxFollow,
) -> Result<SandboxTargetIdentity, Error> {
    target
        .target_identity(follow)
        .map_err(|source| classify_query_error(generation, source))
}

async fn authorize_policy_target<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxTargetIdentity,
) -> Result<(), Error> {
    let read_only_paths = generation
        .initial_files
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, file)| file.permissions == AgentFilePermissions::ReadOnly)
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    for path in read_only_paths {
        let policy = resolve_namespace_target(generation, SandboxPath::at_root(path)).await?;
        let policy = resolved_policy_target(generation, &policy, SandboxFollow::Yes)?;
        if target.matches(&policy) {
            return Err(Error::Access(AccessError::NotPermitted));
        }
    }
    Ok(())
}

async fn execute_open<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    options: OpenOptions,
) -> Result<SandboxOpened, Error> {
    let mut options = options;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox
                .open(target.clone(), sandbox_open_options(options))
                .await
        };
        match result {
            Ok(opened) => return Ok(opened),
            Err(error) => {
                if open_changes_filesystem(options) {
                    match open_postcondition(&generation, &target, options).await {
                        Ok(true) => {
                            debug_assert_eq!(
                                decide_effect(
                                    FailureCause::Guest,
                                    EffectEvidence::DesiredPostconditionSatisfied,
                                    budget,
                                ),
                                EffectDecision::Succeed
                            );
                            options = existing_open_after_postcondition(options);
                            continue;
                        }
                        Err(postcondition_error) if postcondition_error.is_terminal_failure() => {
                            generation.invalidate();
                            return Err(Error::RuntimeInvalidated);
                        }
                        Ok(false) | Err(_) => {}
                    }
                }
                let evidence =
                    if !open_changes_filesystem(options) || error_proves_no_effect(&error) {
                        EffectEvidence::NoEffect
                    } else {
                        EffectEvidence::Unknown {
                            known_completed_prefix: 0,
                        }
                    };
                match decide_generation_effect(&generation, &error, evidence, budget).await {
                    EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
                    EffectDecision::ReturnFailure(cause) => {
                        return Err(classified_error(cause, error));
                    }
                    EffectDecision::Invalidate | EffectDecision::RetryAfterProvenNoEffect => {
                        generation.invalidate();
                        return Err(Error::RuntimeInvalidated);
                    }
                    EffectDecision::ReclaimCapacityThenRetry => {
                        return Err(Error::PhysicalCapacity(error));
                    }
                    EffectDecision::Succeed | EffectDecision::RetryUnwrittenSuffix => {
                        generation.invalidate();
                        return Err(Error::RuntimeInvalidated);
                    }
                }
            }
        }
    }
}

async fn open_postcondition<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxPath,
    options: OpenOptions,
) -> Result<bool, FilesystemStorageError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification(
            "inspect deleted filesystem open postcondition",
            std::path::Path::new("<agent-filesystem>"),
        )
    })?;
    let attributes = sandbox
        .get_path_attributes(
            target.clone(),
            sandbox_follow(match options {
                OpenOptions::Existing { follow, .. } | OpenOptions::File { follow, .. } => follow,
            }),
        )
        .await?;
    Ok(match options {
        OpenOptions::File {
            disposition: FileDisposition::CreateIfMissing,
            ..
        } => attributes.kind == SandboxObjectKind::File,
        OpenOptions::File {
            disposition: FileDisposition::TruncateExisting | FileDisposition::CreateOrTruncate,
            ..
        } => attributes.kind == SandboxObjectKind::File && attributes.size == 0,
        OpenOptions::Existing { .. }
        | OpenOptions::File {
            disposition: FileDisposition::CreateExclusive,
            ..
        } => false,
    })
}

fn open_changes_filesystem(options: OpenOptions) -> bool {
    matches!(options, OpenOptions::File { .. })
}

fn existing_open_after_postcondition(options: OpenOptions) -> OpenOptions {
    match options {
        OpenOptions::File { access, follow, .. } => OpenOptions::Existing {
            expected: ObjectKind::File,
            access,
            follow,
        },
        OpenOptions::Existing { .. } => options,
    }
}

async fn execute_attribute_changes<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: AttributeTarget,
    changes: AttributeChanges,
) -> Result<(), Error> {
    match changes {
        AttributeChanges::Times(times) => execute_set_times(generation, target, times).await,
        AttributeChanges::File { size, times } => {
            let AttributeTarget::Open(SandboxNode::File(file)) = target else {
                return Err(Error::Access(AccessError::WrongGeneration));
            };
            execute_set_size(Arc::clone(&generation), file.clone(), size).await?;
            execute_set_times(
                generation,
                AttributeTarget::Open(SandboxNode::File(file)),
                times,
            )
            .await
        }
    }
}

async fn execute_coordinated_path_attribute_changes<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    follow: Follow,
    changes: AttributeChanges,
) -> Result<(), Error> {
    loop {
        let resolved = resolve_namespace_target(&generation, target.clone()).await?;
        let mut coordination = generation
            .namespace
            .coordinate(
                NamespaceCoordinationKind::Observe,
                vec![resolved.coordination_key()],
            )
            .await;
        if !coordination.extend(Vec::new()) {
            continue;
        }
        let policy_target = resolved_policy_target(&generation, &resolved, sandbox_follow(follow))?;
        authorize_policy_target(&generation, &policy_target).await?;
        return execute_attribute_changes(
            generation,
            AttributeTarget::Path {
                target: resolved.target(),
                follow: sandbox_follow(follow),
            },
            changes,
        )
        .await;
    }
}

async fn execute_set_size<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    file: SandboxFile,
    size: u64,
) -> Result<(), Error> {
    let target = AttributeTarget::Open(SandboxNode::File(file.clone()));
    let before = mutation_attributes(&generation, target.clone()).await?;
    if before.size == size {
        return Ok(());
    }
    let growing = size > before.size;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.set_size(&file, size).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let observed = match mutation_attributes_sandbox(&generation, target.clone()).await {
            Ok(observed) => observed,
            Err(_) => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        };
        let evidence = resize_postcondition_evidence(before.size, observed.size, size);
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        if evidence != EffectEvidence::NoEffect {
            generation.invalidate();
            return Err(Error::RuntimeInvalidated);
        }
        match decide_resize_effect(&generation, &error, evidence, budget, growing).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry
            | EffectDecision::Invalidate
            | EffectDecision::RetryAfterProvenNoEffect
            | EffectDecision::RetryUnwrittenSuffix
            | EffectDecision::Succeed => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_set_times<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: AttributeTarget,
    times: TimeChanges,
) -> Result<(), Error> {
    if time_changes_are_noop(times) {
        return Ok(());
    }
    let before = mutation_attributes(&generation, target.clone()).await?;
    if time_changes_satisfied(&before, &before, times, None) {
        return Ok(());
    }
    let mut budget = RetryBudget::new(2);
    loop {
        let started = std::time::SystemTime::now();
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            set_sandbox_times(
                sandbox.as_ref(),
                target.clone(),
                sandbox_time_changes(times),
            )
            .await
        };
        let finished = std::time::SystemTime::now();
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let observed = match mutation_attributes_sandbox(&generation, target.clone()).await {
            Ok(observed) => observed,
            Err(_) => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        };
        let evidence =
            timestamp_postcondition_evidence(&before, &observed, times, (started, finished));
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        if evidence != EffectEvidence::NoEffect {
            generation.invalidate();
            return Err(Error::RuntimeInvalidated);
        }
        match decide_generation_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry
            | EffectDecision::Invalidate
            | EffectDecision::RetryAfterProvenNoEffect
            | EffectDecision::RetryUnwrittenSuffix
            | EffectDecision::Succeed => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn mutation_attributes<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: AttributeTarget,
) -> Result<SandboxAttributes, Error> {
    mutation_attributes_sandbox(generation, target)
        .await
        .map_err(|source| classify_query_error(generation, source))
}

async fn mutation_attributes_sandbox<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: AttributeTarget,
) -> Result<SandboxAttributes, FilesystemStorageError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification(
            "inspect deleted filesystem attribute postcondition",
            std::path::Path::new("<agent-filesystem>"),
        )
    })?;
    read_sandbox_attributes(sandbox.as_ref(), target).await
}

async fn read_sandbox_attributes<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    target: AttributeTarget,
) -> Result<SandboxAttributes, FilesystemStorageError> {
    match target {
        AttributeTarget::Open(node) => sandbox.get_node_attributes(node).await,
        AttributeTarget::Path { target, follow } => {
            sandbox.get_path_attributes(target, follow).await
        }
    }
}

async fn set_sandbox_times<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    target: AttributeTarget,
    times: SandboxTimeChanges,
) -> Result<(), FilesystemStorageError> {
    match target {
        AttributeTarget::Open(node) => sandbox.set_node_times(node, times).await,
        AttributeTarget::Path { target, follow } => {
            sandbox.set_path_times(target, follow, times).await
        }
    }
}

fn resize_postcondition_evidence(before: u64, observed: u64, requested: u64) -> EffectEvidence {
    if observed == requested {
        EffectEvidence::DesiredPostconditionSatisfied
    } else if observed == before {
        EffectEvidence::NoEffect
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

fn timestamp_postcondition_evidence(
    before: &SandboxAttributes,
    observed: &SandboxAttributes,
    requested: TimeChanges,
    now_range: (std::time::SystemTime, std::time::SystemTime),
) -> EffectEvidence {
    if time_changes_satisfied(before, observed, requested, Some(now_range)) {
        EffectEvidence::DesiredPostconditionSatisfied
    } else if observed.accessed == before.accessed && observed.modified == before.modified {
        EffectEvidence::NoEffect
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

fn time_changes_satisfied(
    before: &SandboxAttributes,
    observed: &SandboxAttributes,
    requested: TimeChanges,
    now_range: Option<(std::time::SystemTime, std::time::SystemTime)>,
) -> bool {
    time_change_satisfied(
        before.accessed,
        observed.accessed,
        requested.accessed,
        now_range,
    ) && time_change_satisfied(
        before.modified,
        observed.modified,
        requested.modified,
        now_range,
    )
}

fn time_change_satisfied(
    before: Option<std::time::SystemTime>,
    observed: Option<std::time::SystemTime>,
    requested: TimeChange,
    now_range: Option<(std::time::SystemTime, std::time::SystemTime)>,
) -> bool {
    match requested {
        TimeChange::Keep => observed == before,
        TimeChange::Now => now_range.is_some_and(|(started, finished)| {
            observed.is_some_and(|observed| observed >= started && observed <= finished)
        }),
        TimeChange::Set(requested) => observed == Some(requested),
    }
}

fn time_changes_are_noop(changes: TimeChanges) -> bool {
    changes.accessed == TimeChange::Keep && changes.modified == TimeChange::Keep
}

async fn execute_write<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    file: SandboxFile,
    placement: WritePlacement,
    bytes: Bytes,
) -> Result<WriteResult, Error> {
    let _append: Option<tokio::sync::OwnedMutexGuard<()>> = match placement {
        WritePlacement::At(_) => None,
        WritePlacement::Append => Some(file.coordinate_append().await),
    };
    let mut completed = 0u64;
    let mut budget = RetryBudget::new(2);
    while completed < bytes.len() as u64 {
        let (sandbox_placement, remaining) = unwritten_write_suffix(placement, &bytes, completed)?;
        let attempt = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.write(&file, sandbox_placement, remaining).await
        };
        let SandboxWriteAttempt { written, result } = match attempt {
            Ok(attempt) => attempt,
            Err(_) => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        };
        completed = completed
            .checked_add(written)
            .filter(|completed| *completed <= bytes.len() as u64)
            .ok_or_else(|| {
                generation.invalidate();
                Error::RuntimeInvalidated
            })?;
        let error = match result {
            Ok(()) if completed == bytes.len() as u64 => break,
            Ok(()) if written != 0 => continue,
            Ok(()) => FilesystemStorageError::io(
                "write sandbox filesystem file",
                std::path::Path::new("<agent-filesystem>"),
                std::io::ErrorKind::WriteZero.into(),
            ),
            Err(error) => error,
        };
        let evidence = NonZeroU64::new(completed)
            .map_or(EffectEvidence::NoEffect, EffectEvidence::CompletedPrefix);
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect | EffectDecision::RetryUnwrittenSuffix
                if budget.consume() =>
            {
                continue;
            }
            EffectDecision::ReturnFailure(_) if completed != 0 => {
                return Ok(WriteResult { written: completed });
            }
            EffectDecision::ReturnFailure(cause) => {
                return Err(classified_error(cause, error));
            }
            EffectDecision::Invalidate
            | EffectDecision::RetryAfterProvenNoEffect
            | EffectDecision::RetryUnwrittenSuffix => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            EffectDecision::Succeed => return Ok(WriteResult { written: completed }),
        }
    }
    Ok(WriteResult { written: completed })
}

fn unwritten_write_suffix(
    placement: WritePlacement,
    bytes: &Bytes,
    completed: u64,
) -> Result<(SandboxWritePlacement, Bytes), Error> {
    let offset = usize::try_from(completed).map_err(|_| Error::RuntimeInvalidated)?;
    if offset > bytes.len() {
        return Err(Error::RuntimeInvalidated);
    }
    let remaining = bytes.slice(offset..);
    let placement = match placement {
        WritePlacement::At(initial) => SandboxWritePlacement::At(
            initial
                .checked_add(completed)
                .ok_or(Error::RuntimeInvalidated)?,
        ),
        WritePlacement::Append => SandboxWritePlacement::Append,
    };
    Ok((placement, remaining))
}

async fn execute_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    edit: NamespaceEdit,
) -> Result<(), Error> {
    loop {
        let resolved = resolve_namespace_edit(&generation, &edit).await?;
        let mut coordination = generation
            .namespace
            .coordinate(
                NamespaceCoordinationKind::Edit,
                resolved.coordination_keys(),
            )
            .await;
        authorize_resolved_namespace_edit(&generation, &resolved).await?;
        let directory_keys = refresh_namespace_directory_keys(&generation, &resolved).await?;
        if !coordination.extend(directory_keys) {
            continue;
        }
        return execute_resolved_namespace_edit(generation, resolved).await;
    }
}

enum ResolvedNamespaceEdit {
    Insert {
        destination: SandboxResolvedNamespaceTarget,
        object: NewObject,
    },
    Link {
        source: SandboxResolvedNamespaceTarget,
        destination: SandboxResolvedNamespaceTarget,
    },
    Move {
        source: SandboxResolvedNamespaceTarget,
        destination: SandboxResolvedNamespaceTarget,
    },
    Remove {
        target: SandboxResolvedNamespaceTarget,
        expected: ObjectKind,
    },
}

async fn authorize_resolved_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    edit: &ResolvedNamespaceEdit,
) -> Result<(), Error> {
    let targets = match edit {
        ResolvedNamespaceEdit::Insert { destination, .. } => vec![destination],
        ResolvedNamespaceEdit::Link {
            source,
            destination,
        }
        | ResolvedNamespaceEdit::Move {
            source,
            destination,
        } => vec![source, destination],
        ResolvedNamespaceEdit::Remove { target, .. } => vec![target],
    };
    for target in targets {
        let target = resolved_policy_target(generation, target, SandboxFollow::No)?;
        authorize_policy_target(generation, &target).await?;
    }
    Ok(())
}

impl ResolvedNamespaceEdit {
    fn coordination_keys(&self) -> Vec<SandboxNamespaceCoordinationKey> {
        match self {
            Self::Insert { destination, .. } => vec![destination.coordination_key()],
            Self::Link {
                source,
                destination,
            }
            | Self::Move {
                source,
                destination,
            } => vec![source.coordination_key(), destination.coordination_key()],
            Self::Remove { target, .. } => vec![target.coordination_key()],
        }
    }
}

async fn resolve_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    edit: &NamespaceEdit,
) -> Result<ResolvedNamespaceEdit, Error> {
    Ok(match edit {
        NamespaceEdit::Insert {
            destination,
            object,
        } => ResolvedNamespaceEdit::Insert {
            destination: resolve_namespace_target(generation, destination.sandbox.clone()).await?,
            object: object.clone(),
        },
        NamespaceEdit::Link {
            source,
            destination,
        } => ResolvedNamespaceEdit::Link {
            source: resolve_namespace_target(generation, source.sandbox.clone()).await?,
            destination: resolve_namespace_target(generation, destination.sandbox.clone()).await?,
        },
        NamespaceEdit::Move {
            source,
            destination,
        } => ResolvedNamespaceEdit::Move {
            source: resolve_namespace_target(generation, source.sandbox.clone()).await?,
            destination: resolve_namespace_target(generation, destination.sandbox.clone()).await?,
        },
        NamespaceEdit::Remove { target, expected } => ResolvedNamespaceEdit::Remove {
            target: resolve_namespace_target(generation, target.sandbox.clone()).await?,
            expected: *expected,
        },
    })
}

async fn refresh_namespace_directory_keys<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    edit: &ResolvedNamespaceEdit,
) -> Result<Vec<SandboxDirectoryCoordinationKey>, Error> {
    let targets = match edit {
        ResolvedNamespaceEdit::Move {
            source,
            destination,
        } => vec![source, destination],
        ResolvedNamespaceEdit::Remove { target, .. } => vec![target],
        ResolvedNamespaceEdit::Insert { .. } | ResolvedNamespaceEdit::Link { .. } => Vec::new(),
    };
    let mut keys = Vec::new();
    for target in targets {
        if let Some(key) = resolve_namespace_target(generation, target.target())
            .await?
            .final_directory_key()
        {
            keys.push(key);
        }
    }
    Ok(keys)
}

async fn execute_resolved_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    edit: ResolvedNamespaceEdit,
) -> Result<(), Error> {
    match edit {
        ResolvedNamespaceEdit::Insert {
            destination,
            object,
        } => match object {
            NewObject::Directory => {
                execute_create_directory(generation, destination.target()).await
            }
            NewObject::Symlink(target) => {
                execute_create_symlink(
                    generation,
                    destination.target(),
                    SandboxSymlinkTarget(target.0),
                )
                .await
            }
        },
        ResolvedNamespaceEdit::Link {
            source,
            destination,
        } => execute_hard_link(generation, source.target(), destination.target()).await,
        ResolvedNamespaceEdit::Move {
            source,
            destination,
        } => execute_rename(generation, source.target(), destination.target()).await,
        ResolvedNamespaceEdit::Remove { target, expected } => {
            execute_remove(generation, target.target(), expected).await
        }
    }
}

async fn execute_create_directory<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
) -> Result<(), Error> {
    let before = path_kind_postcondition(&generation, target.clone(), SandboxObjectKind::Directory)
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.create_directory(target.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match path_kind_postcondition(&generation, target.clone(), SandboxObjectKind::Directory)
                .await
            {
                Ok(after) => insert_postcondition_evidence(&before, &after),
                Err(postcondition_error) if postcondition_error.is_terminal_failure() => {
                    generation.invalidate();
                    return Err(Error::RuntimeInvalidated);
                }
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_create_symlink<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    desired: SandboxSymlinkTarget,
) -> Result<(), Error> {
    let before = symlink_postcondition(&generation, target.clone(), &desired)
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox
                .create_symlink(target.clone(), desired.clone())
                .await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match symlink_postcondition(&generation, target.clone(), &desired).await {
                Ok(after) => insert_postcondition_evidence(&before, &after),
                Err(postcondition_error) if postcondition_error.is_terminal_failure() => {
                    generation.invalidate();
                    return Err(Error::RuntimeInvalidated);
                }
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_hard_link<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    source: SandboxPath,
    destination: SandboxPath,
) -> Result<(), Error> {
    let destination_before = namespace_path_state(&generation, destination.clone())
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.hard_link(source.clone(), destination.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match namespace_path_state(&generation, destination.clone()).await {
                Ok(destination_after) => {
                    hard_link_postcondition_evidence(&destination_before, &destination_after)
                }
                Err(postcondition_error) if postcondition_error.is_terminal_failure() => {
                    generation.invalidate();
                    return Err(Error::RuntimeInvalidated);
                }
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_rename<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    source: SandboxPath,
    destination: SandboxPath,
) -> Result<(), Error> {
    let source_before = namespace_path_state(&generation, source.clone())
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.rename(source.clone(), destination.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            let source_after = namespace_path_state(&generation, source.clone()).await;
            let destination_after = namespace_path_state(&generation, destination.clone()).await;
            match (source_after, destination_after) {
                (Ok(source_after), Ok(destination_after)) => {
                    move_postcondition_evidence(&source_before, &source_after, &destination_after)
                }
                (Err(postcondition_error), _) | (_, Err(postcondition_error))
                    if postcondition_error.is_terminal_failure() =>
                {
                    generation.invalidate();
                    return Err(Error::RuntimeInvalidated);
                }
                _ => EffectEvidence::Unknown {
                    known_completed_prefix: 0,
                },
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_remove<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    expected: ObjectKind,
) -> Result<(), Error> {
    let before = namespace_path_state(&generation, target.clone())
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    match &before {
        NamespacePathState::Absent => {}
        NamespacePathState::Present(attributes)
            if !remove_kind_matches(expected, attributes.kind) =>
        {
            return Err(Error::Sandbox(namespace_kind_mismatch_error(
                expected,
                attributes.kind,
            )));
        }
        NamespacePathState::Present(_) => {}
    }
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            match expected {
                ObjectKind::Directory => sandbox.remove_directory(target.clone()).await,
                ObjectKind::File | ObjectKind::Symlink => sandbox.unlink_file(target.clone()).await,
            }
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match namespace_path_state(&generation, target.clone()).await {
                Ok(after) => remove_postcondition_evidence(&before, &after, expected),
                Err(postcondition_error) if postcondition_error.is_terminal_failure() => {
                    generation.invalidate();
                    return Err(Error::RuntimeInvalidated);
                }
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_generation_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

fn remove_kind_matches(expected: ObjectKind, actual: SandboxObjectKind) -> bool {
    match expected {
        ObjectKind::File => matches!(actual, SandboxObjectKind::File | SandboxObjectKind::Symlink),
        ObjectKind::Directory => actual == SandboxObjectKind::Directory,
        ObjectKind::Symlink => actual == SandboxObjectKind::Symlink,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InsertPostcondition {
    path: NamespacePathState,
    desired: bool,
}

async fn path_kind_postcondition<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
    expected: SandboxObjectKind,
) -> Result<InsertPostcondition, FilesystemStorageError> {
    let path = namespace_path_state(generation, target).await?;
    let desired = matches!(
        &path,
        NamespacePathState::Present(attributes) if attributes.kind == expected
    );
    Ok(InsertPostcondition { path, desired })
}

async fn symlink_postcondition<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
    expected: &SandboxSymlinkTarget,
) -> Result<InsertPostcondition, FilesystemStorageError> {
    let path_state = namespace_path_state(generation, target.clone()).await?;
    match &path_state {
        NamespacePathState::Absent => {
            return Ok(InsertPostcondition {
                path: path_state,
                desired: false,
            });
        }
        NamespacePathState::Present(attributes)
            if attributes.kind != SandboxObjectKind::Symlink =>
        {
            return Ok(InsertPostcondition {
                path: path_state,
                desired: false,
            });
        }
        NamespacePathState::Present(_) => {}
    }
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification(
            "inspect deleted filesystem symlink postcondition",
            std::path::Path::new("<agent-filesystem>"),
        )
    })?;
    let desired = sandbox
        .read_link(target)
        .await
        .map(|observed| observed == *expected)?;
    Ok(InsertPostcondition {
        path: path_state,
        desired,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NamespacePathState {
    Absent,
    Present(SandboxAttributes),
}

fn insert_postcondition_evidence(
    before: &InsertPostcondition,
    after: &InsertPostcondition,
) -> EffectEvidence {
    if before == after {
        EffectEvidence::NoEffect
    } else if matches!(before.path, NamespacePathState::Absent) && after.desired {
        EffectEvidence::DesiredPostconditionSatisfied
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

fn hard_link_postcondition_evidence(
    before: &NamespacePathState,
    after: &NamespacePathState,
) -> EffectEvidence {
    match (before, after) {
        (NamespacePathState::Absent, NamespacePathState::Absent)
        | (NamespacePathState::Present(_), NamespacePathState::Present(_)) => {
            EffectEvidence::NoEffect
        }
        _ => EffectEvidence::Unknown {
            known_completed_prefix: 0,
        },
    }
}

fn remove_postcondition_evidence(
    before: &NamespacePathState,
    after: &NamespacePathState,
    expected: ObjectKind,
) -> EffectEvidence {
    match (before, after) {
        (NamespacePathState::Present(attributes), NamespacePathState::Absent)
            if attributes.kind == sandbox_object_kind(expected) =>
        {
            EffectEvidence::DesiredPostconditionSatisfied
        }
        (NamespacePathState::Absent, NamespacePathState::Absent) => EffectEvidence::NoEffect,
        (NamespacePathState::Present(before), NamespacePathState::Present(after))
            if before.kind == sandbox_object_kind(expected)
                && after.kind == sandbox_object_kind(expected) =>
        {
            EffectEvidence::NoEffect
        }
        _ => EffectEvidence::Unknown {
            known_completed_prefix: 0,
        },
    }
}

async fn namespace_path_state<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
) -> Result<NamespacePathState, FilesystemStorageError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification(
            "inspect deleted filesystem namespace postcondition",
            std::path::Path::new("<agent-filesystem>"),
        )
    })?;
    match sandbox.get_path_attributes(target, SandboxFollow::No).await {
        Ok(attributes) => Ok(NamespacePathState::Present(attributes)),
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
            Ok(NamespacePathState::Absent)
        }
        Err(error) => Err(error),
    }
}

fn move_postcondition_evidence(
    source_before: &NamespacePathState,
    source_after: &NamespacePathState,
    destination_after: &NamespacePathState,
) -> EffectEvidence {
    match (source_before, source_after, destination_after) {
        (
            NamespacePathState::Present(_),
            NamespacePathState::Absent,
            NamespacePathState::Present(_),
        ) => EffectEvidence::DesiredPostconditionSatisfied,
        (NamespacePathState::Absent, _, _)
        | (NamespacePathState::Present(_), NamespacePathState::Present(_), _) => {
            EffectEvidence::NoEffect
        }
        _ => EffectEvidence::Unknown {
            known_completed_prefix: 0,
        },
    }
}

fn namespace_kind_mismatch_error(
    expected: ObjectKind,
    observed: SandboxObjectKind,
) -> FilesystemStorageError {
    let kind = if observed == SandboxObjectKind::Directory {
        std::io::ErrorKind::IsADirectory
    } else if expected == ObjectKind::Directory {
        std::io::ErrorKind::NotADirectory
    } else {
        std::io::ErrorKind::InvalidInput
    };
    FilesystemStorageError::io(
        "validate namespace object kind",
        std::path::Path::new("<agent-filesystem>"),
        std::io::Error::new(
            kind,
            format!("expected {expected:?}, observed {observed:?}"),
        ),
    )
}

async fn execute_synchronize<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    node: SandboxNode,
    level: Synchronization,
) -> Result<(), Error> {
    let result = {
        let sandbox = generation.sandbox.read().await;
        let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
        sandbox
            .synchronize(&node, sandbox_synchronization(level))
            .await
    };
    match result {
        Ok(()) => Ok(()),
        Err(_) => {
            generation.invalidate();
            Err(Error::RuntimeInvalidated)
        }
    }
}

async fn execute_release<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    node: SandboxNode,
) -> Result<(), Error> {
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.release(node.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        match decide_generation_effect(
            &generation,
            &error,
            mutation_failure_evidence(&error),
            budget,
        )
        .await
        {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(Error::PhysicalCapacity(error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

fn mutation_failure_evidence(error: &FilesystemStorageError) -> EffectEvidence {
    if error_proves_no_effect(error) {
        EffectEvidence::NoEffect
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

fn classify_query_error<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    source: FilesystemStorageError,
) -> Error {
    if source.is_terminal_failure() {
        generation.invalidate();
        Error::RuntimeInvalidated
    } else {
        Error::Sandbox(source)
    }
}

fn agent_attributes(attributes: SandboxAttributes) -> Attributes {
    Attributes {
        kind: agent_object_kind(attributes.kind),
        link_count: attributes.link_count,
        size: attributes.size,
        accessed: attributes.accessed,
        modified: attributes.modified,
    }
}

fn agent_object_kind(kind: SandboxObjectKind) -> ObjectKind {
    match kind {
        SandboxObjectKind::File => ObjectKind::File,
        SandboxObjectKind::Directory => ObjectKind::Directory,
        SandboxObjectKind::Symlink => ObjectKind::Symlink,
    }
}

async fn decide_generation_effect<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    error: &FilesystemStorageError,
    evidence: EffectEvidence,
    budget: RetryBudget,
) -> EffectDecision {
    let mut decision_budget = budget;
    let cause = if error.is_storage_exhaustion() {
        match generation.observe_usage_sandbox().await {
            Ok(FilesystemUsage::Unsupported) => {
                decision_budget = RetryBudget::new(0);
                FailureCause::UnclassifiedIo
            }
            Ok(FilesystemUsage::Authoritative {
                allocated_bytes,
                filesystem_objects,
            }) => {
                let quota_exhausted = match *generation.limits.lock().unwrap() {
                    ResolvedStorageLimits::Unlimited => false,
                    ResolvedStorageLimits::Finite(limits) => {
                        allocated_bytes >= limits.allocated_bytes
                            || filesystem_objects >= limits.filesystem_objects
                    }
                };
                if quota_exhausted {
                    FailureCause::AgentQuota
                } else {
                    decision_budget = RetryBudget::new(0);
                    FailureCause::UnclassifiedIo
                }
            }
            Err(observation_error) if observation_error.is_terminal_failure() => {
                FailureCause::TerminalInfrastructure
            }
            Err(_) => {
                decision_budget = RetryBudget::new(0);
                FailureCause::UnclassifiedIo
            }
        }
    } else {
        classify_failure(error, FailureFacts::default())
    };
    decide_effect(cause, evidence, decision_budget)
}

async fn decide_write_effect<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    error: &FilesystemStorageError,
    evidence: EffectEvidence,
    budget: RetryBudget,
) -> EffectDecision {
    if !error.is_storage_exhaustion() {
        return decide_effect(
            classify_failure(error, FailureFacts::default()),
            evidence,
            budget,
        );
    }

    let quota_exhausted = match generation.observe_usage_sandbox().await {
        Ok(FilesystemUsage::Unsupported) => {
            return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo);
        }
        Ok(FilesystemUsage::Authoritative {
            allocated_bytes,
            filesystem_objects,
        }) => match *generation.limits.lock().unwrap() {
            ResolvedStorageLimits::Unlimited => false,
            ResolvedStorageLimits::Finite(limits) => {
                allocated_bytes >= limits.allocated_bytes
                    || filesystem_objects >= limits.filesystem_objects
                    || error.io_kind() == Some(std::io::ErrorKind::QuotaExceeded)
            }
        },
        Err(error) if error.is_terminal_failure() => {
            return EffectDecision::Invalidate;
        }
        Err(_) => return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo),
    };
    if quota_exhausted {
        return EffectDecision::ReturnFailure(FailureCause::AgentQuota);
    }

    let retry = match evidence {
        EffectEvidence::NoEffect if budget.available() => EffectDecision::RetryAfterProvenNoEffect,
        EffectEvidence::CompletedPrefix(_) if budget.available() => {
            EffectDecision::RetryUnwrittenSuffix
        }
        EffectEvidence::NoEffect | EffectEvidence::CompletedPrefix(_) => {
            return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo);
        }
        EffectEvidence::DesiredPostconditionSatisfied => return EffectDecision::Succeed,
        EffectEvidence::Unknown { .. } => return EffectDecision::Invalidate,
    };
    let Some(recovery) = &generation.pressure_recovery else {
        return EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo);
    };
    let deadline = std::time::Instant::now() + WRITE_PRESSURE_RECOVERY_TIMEOUT;
    match recovery.recover_write(deadline).await {
        FilesystemWriteRecoveryOutcome::Recovered => retry,
        FilesystemWriteRecoveryOutcome::Denied => {
            EffectDecision::ReturnFailure(FailureCause::PhysicalCapacity)
        }
        FilesystemWriteRecoveryOutcome::NotUnderPressure
        | FilesystemWriteRecoveryOutcome::Unavailable => {
            EffectDecision::ReturnFailure(FailureCause::UnclassifiedIo)
        }
    }
}

async fn decide_resize_effect<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    error: &FilesystemStorageError,
    evidence: EffectEvidence,
    budget: RetryBudget,
    growing: bool,
) -> EffectDecision {
    if growing {
        decide_write_effect(generation, error, evidence, budget).await
    } else {
        decide_generation_effect(generation, error, evidence, budget).await
    }
}

fn classified_error(cause: FailureCause, source: FilesystemStorageError) -> Error {
    match cause {
        FailureCause::AgentQuota => Error::AgentQuota(source),
        FailureCause::PhysicalCapacity => Error::PhysicalCapacity(source),
        FailureCause::TerminalInfrastructure => Error::RuntimeInvalidated,
        FailureCause::Guest | FailureCause::TransientBackend | FailureCause::UnclassifiedIo => {
            Error::Sandbox(source)
        }
    }
}

fn admit<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
) -> Result<Arc<FilesystemGeneration<Adapter>>, AccessError> {
    let generation = generation_handle
        .generation
        .upgrade()
        .ok_or(AccessError::Revoked)?;
    if generation_handle.phase == GenerationHandlePhase::Reconstruction
        && !generation.registry.replay_access_allowed()
    {
        return Err(AccessError::Revoked);
    }
    Ok(generation)
}

fn admit_node<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    generation_id: usize,
) -> Result<Arc<FilesystemGeneration<Adapter>>, AccessError> {
    let generation = admit(generation_handle)?;
    if Arc::as_ptr(&generation) as usize != generation_id {
        return Err(AccessError::WrongGeneration);
    }
    Ok(generation)
}

fn node_ownership(node: &OpenNode) -> &NodeOwnership {
    match node {
        OpenNode::File(file) => &file.ownership,
        OpenNode::Directory(directory) => &directory.ownership,
    }
}

fn authorize_attribute_target(
    target: &Target<'_>,
    changes: AttributeChanges,
) -> Result<(), AccessError> {
    let writable = match target {
        Target::Open(node) => node_ownership(node).access != AccessMode::Read,
        Target::Path(target, _) => target.access != AccessMode::Read,
    };
    if !writable {
        return Err(AccessError::NotPermitted);
    }
    if matches!(changes, AttributeChanges::File { .. })
        && !matches!(target, &Target::Open(OpenNode::File(_)))
    {
        return Err(AccessError::WrongGeneration);
    }
    Ok(())
}

fn sandbox_target<Adapter: SandboxFilesystemAdapter>(
    generation: &Arc<FilesystemGeneration<Adapter>>,
    target: Target<'_>,
) -> Result<AttributeTarget, AccessError> {
    match target {
        Target::Open(node) => {
            let ownership = node_ownership(node);
            if ownership.generation_id != Arc::as_ptr(generation) as usize {
                return Err(AccessError::WrongGeneration);
            }
            Ok(AttributeTarget::Open(ownership.sandbox().clone()))
        }
        Target::Path(target, follow) => Ok(AttributeTarget::Path {
            target: {
                validate_path_generation(generation, target)?;
                target.sandbox.clone()
            },
            follow: sandbox_follow(follow),
        }),
    }
}

fn validate_namespace_generation<Adapter: SandboxFilesystemAdapter>(
    generation: &Arc<FilesystemGeneration<Adapter>>,
    edit: &NamespaceEdit,
) -> Result<(), AccessError> {
    match edit {
        NamespaceEdit::Insert { destination, .. }
        | NamespaceEdit::Remove {
            target: destination,
            ..
        } => validate_path_generation(generation, destination),
        NamespaceEdit::Link {
            source,
            destination,
        }
        | NamespaceEdit::Move {
            source,
            destination,
        } => {
            validate_path_generation(generation, source)?;
            validate_path_generation(generation, destination)
        }
    }
}

fn authorize_namespace_edit(edit: &NamespaceEdit) -> Result<(), AccessError> {
    let writable = |target: &PathTarget| target.access != AccessMode::Read;
    let permitted = match edit {
        NamespaceEdit::Insert { destination, .. } => writable(destination),
        NamespaceEdit::Link {
            source,
            destination,
        }
        | NamespaceEdit::Move {
            source,
            destination,
        } => writable(source) && writable(destination),
        NamespaceEdit::Remove { target, .. } => writable(target),
    };
    if permitted {
        Ok(())
    } else {
        Err(AccessError::NotPermitted)
    }
}

fn validate_path_generation<Adapter: SandboxFilesystemAdapter>(
    generation: &Arc<FilesystemGeneration<Adapter>>,
    target: &PathTarget,
) -> Result<(), AccessError> {
    if Arc::as_ptr(generation) as usize == target.generation_id {
        Ok(())
    } else {
        Err(AccessError::WrongGeneration)
    }
}

fn open_returns_directory(options: OpenOptions) -> bool {
    matches!(
        options,
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            ..
        }
    )
}

fn open_namespace_coordination(options: OpenOptions) -> Option<NamespaceCoordinationKind> {
    match options {
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            ..
        } => Some(NamespaceCoordinationKind::Observe),
        OpenOptions::Existing { access, .. } if access != AccessMode::Read => {
            Some(NamespaceCoordinationKind::Observe)
        }
        OpenOptions::File { .. } => Some(NamespaceCoordinationKind::Edit),
        OpenOptions::Existing { .. } => None,
    }
}

fn open_requires_mutable_target(options: OpenOptions) -> bool {
    match options {
        OpenOptions::Existing { access, .. } => access != AccessMode::Read,
        OpenOptions::File { .. } => true,
    }
}

fn open_access(options: OpenOptions) -> AccessMode {
    match options {
        OpenOptions::Existing { access, .. } | OpenOptions::File { access, .. } => access,
    }
}

fn sandbox_open_options(options: OpenOptions) -> SandboxOpenOptions {
    match options {
        OpenOptions::Existing {
            expected,
            access,
            follow,
        } => SandboxOpenOptions::Existing {
            expected: sandbox_object_kind(expected),
            access: sandbox_access_mode(access),
            follow: sandbox_follow(follow),
        },
        OpenOptions::File {
            access,
            disposition,
            follow,
        } => SandboxOpenOptions::File {
            access: sandbox_access_mode(access),
            disposition: match disposition {
                FileDisposition::CreateIfMissing => SandboxFileDisposition::CreateIfMissing,
                FileDisposition::CreateExclusive => SandboxFileDisposition::CreateExclusive,
                FileDisposition::TruncateExisting => SandboxFileDisposition::TruncateExisting,
                FileDisposition::CreateOrTruncate => SandboxFileDisposition::CreateOrTruncate,
            },
            follow: sandbox_follow(follow),
        },
    }
}

fn sandbox_object_kind(kind: ObjectKind) -> SandboxObjectKind {
    match kind {
        ObjectKind::File => SandboxObjectKind::File,
        ObjectKind::Directory => SandboxObjectKind::Directory,
        ObjectKind::Symlink => SandboxObjectKind::Symlink,
    }
}

fn sandbox_access_mode(mode: AccessMode) -> SandboxAccessMode {
    match mode {
        AccessMode::Read => SandboxAccessMode::Read,
        AccessMode::Write => SandboxAccessMode::Write,
        AccessMode::ReadWrite => SandboxAccessMode::ReadWrite,
    }
}

fn sandbox_file_permissions(read_only: bool) -> SandboxFilePermissions {
    if read_only {
        SandboxFilePermissions::ReadOnly
    } else {
        SandboxFilePermissions::ReadWrite
    }
}

fn sandbox_follow(follow: Follow) -> SandboxFollow {
    match follow {
        Follow::Yes => SandboxFollow::Yes,
        Follow::No => SandboxFollow::No,
    }
}

fn sandbox_synchronization(level: Synchronization) -> SandboxSynchronization {
    match level {
        Synchronization::Data => SandboxSynchronization::Data,
        Synchronization::DataAndMetadata => SandboxSynchronization::DataAndMetadata,
    }
}

fn sandbox_time_changes(times: TimeChanges) -> SandboxTimeChanges {
    SandboxTimeChanges {
        accessed: sandbox_time_change(times.accessed),
        modified: sandbox_time_change(times.modified),
    }
}

fn sandbox_time_change(change: TimeChange) -> SandboxTimeChange {
    match change {
        TimeChange::Keep => SandboxTimeChange::Keep,
        TimeChange::Now => SandboxTimeChange::Now,
        TimeChange::Set(time) => SandboxTimeChange::Set(time),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectEvidence {
    NoEffect,
    CompletedPrefix(NonZeroU64),
    DesiredPostconditionSatisfied,
    Unknown { known_completed_prefix: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureCause {
    Guest,
    AgentQuota,
    PhysicalCapacity,
    TransientBackend,
    UnclassifiedIo,
    TerminalInfrastructure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectDecision {
    Succeed,
    ReturnFailure(FailureCause),
    RetryUnwrittenSuffix,
    RetryAfterProvenNoEffect,
    ReclaimCapacityThenRetry,
    Invalidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RetryBudget {
    remaining: u8,
}

impl RetryBudget {
    fn new(remaining: u8) -> Self {
        Self { remaining }
    }

    fn available(self) -> bool {
        self.remaining != 0
    }

    fn consume(&mut self) -> bool {
        if self.remaining == 0 {
            false
        } else {
            self.remaining -= 1;
            true
        }
    }
}

fn decide_effect(
    cause: FailureCause,
    evidence: EffectEvidence,
    budget: RetryBudget,
) -> EffectDecision {
    match evidence {
        EffectEvidence::DesiredPostconditionSatisfied => EffectDecision::Succeed,
        EffectEvidence::Unknown { .. } => EffectDecision::Invalidate,
        _ if cause == FailureCause::TerminalInfrastructure => EffectDecision::Invalidate,
        EffectEvidence::CompletedPrefix(_)
            if budget.available()
                && matches!(
                    cause,
                    FailureCause::TransientBackend | FailureCause::UnclassifiedIo
                ) =>
        {
            EffectDecision::RetryUnwrittenSuffix
        }
        EffectEvidence::CompletedPrefix(_) => EffectDecision::ReturnFailure(cause),
        EffectEvidence::NoEffect => match cause {
            FailureCause::PhysicalCapacity if budget.available() => {
                EffectDecision::ReclaimCapacityThenRetry
            }
            FailureCause::TransientBackend | FailureCause::UnclassifiedIo if budget.available() => {
                EffectDecision::RetryAfterProvenNoEffect
            }
            FailureCause::TransientBackend | FailureCause::UnclassifiedIo => {
                EffectDecision::ReturnFailure(cause)
            }
            FailureCause::TerminalInfrastructure => EffectDecision::Invalidate,
            FailureCause::Guest | FailureCause::AgentQuota | FailureCause::PhysicalCapacity => {
                EffectDecision::ReturnFailure(cause)
            }
        },
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FailureFacts {
    quota_exhausted: bool,
    physical_capacity_exhausted: bool,
}

fn classify_failure(error: &FilesystemStorageError, facts: FailureFacts) -> FailureCause {
    if error.is_terminal_failure() {
        FailureCause::TerminalInfrastructure
    } else if error.is_storage_exhaustion() && facts.quota_exhausted {
        FailureCause::AgentQuota
    } else if error.is_storage_exhaustion() && facts.physical_capacity_exhausted {
        FailureCause::PhysicalCapacity
    } else {
        match error.io_kind() {
            Some(std::io::ErrorKind::NotFound)
            | Some(std::io::ErrorKind::AlreadyExists)
            | Some(std::io::ErrorKind::InvalidInput)
            | Some(std::io::ErrorKind::InvalidFilename)
            | Some(std::io::ErrorKind::IsADirectory)
            | Some(std::io::ErrorKind::NotADirectory)
            | Some(std::io::ErrorKind::CrossesDevices) => FailureCause::Guest,
            Some(std::io::ErrorKind::WouldBlock) | Some(std::io::ErrorKind::Interrupted) => {
                FailureCause::TransientBackend
            }
            _ => FailureCause::UnclassifiedIo,
        }
    }
}

fn error_proves_no_effect(error: &FilesystemStorageError) -> bool {
    matches!(
        error.io_kind(),
        Some(std::io::ErrorKind::WouldBlock)
            | Some(std::io::ErrorKind::NotFound)
            | Some(std::io::ErrorKind::AlreadyExists)
            | Some(std::io::ErrorKind::InvalidInput)
            | Some(std::io::ErrorKind::InvalidFilename)
            | Some(std::io::ErrorKind::IsADirectory)
            | Some(std::io::ErrorKind::NotADirectory)
            | Some(std::io::ErrorKind::CrossesDevices)
            | Some(std::io::ErrorKind::StorageFull)
            | Some(std::io::ErrorKind::QuotaExceeded)
    )
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(all(test, target_os = "linux"))]
mod workload_benchmark;
