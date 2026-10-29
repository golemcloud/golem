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

use crate::filesystem_error::FilesystemOperationError;
use crate::filesystem_pressure::{FilesystemWriteRecovery, FilesystemWriteRecoveryOutcome};
use crate::host_directories::HostDirectory;
use crate::metrics::agent_filesystem::{self, StorageRefusal};
#[cfg(test)]
use crate::sandbox_filesystem::ScriptedSandboxFilesystem;
use crate::sandbox_filesystem::{
    FilesystemStorageError, InstalledLimits, LinkGroup, SandboxAccessMode, SandboxAttributes,
    SandboxDirectoryId, SandboxFile, SandboxFileDisposition, SandboxFilesystem,
    SandboxFilesystemAdapter, SandboxFilesystemName, SandboxFilesystemProvisioning,
    SandboxFlushLevel, SandboxFollow, SandboxHandle, SandboxNameComparisonKey, SandboxObjectId,
    SandboxObjectKind, SandboxOpenOptions, SandboxPath, SandboxReadRange,
    SandboxResolvedPathTarget, SandboxSymlinkTarget, SandboxTimeChange, SandboxTimeChanges,
    SandboxWriteAttempt, SandboxWritePlacement, SeedAccess, SeedEntry, SeedPlacement,
    TreeExclusions,
};
use crate::services::active_agents::ConcurrentAgentPermit;
use crate::services::file_loader::{
    FileLoader, InitialFileLoadError, InitialFileLoadFailure, InitialFileSource,
};
use crate::services::resource_usage_metering::{
    FilesystemUsage, FilesystemUsageReader, FilesystemUsageSource, MeteringOpenError,
    ResourceUsageAccount, ResourceUsageMeter, ResourceUsageMeteringWindow, create_unbound_meter,
    install_filesystem_usage, open_window, stop_metering,
};
use crate::storage_volume::AgentAccounting;
use crate::storage_volume::FilesystemLimits;
use bytes::Bytes;
use futures::future::BoxFuture;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::component::{AgentFilePermissions, InitialAgentFile};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::{BlobRefusal, agent_path_segment};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

mod attributes;
mod effects;
mod handles;
mod namespace;
mod operations;
mod registry;
mod transitions;
use attributes::read_sandbox_attributes;
#[cfg(test)]
use attributes::resize_postcondition_evidence;
pub(crate) use attributes::restore_times;
pub(crate) use attributes::set_attributes;
#[cfg(test)]
use attributes::set_times_change;
#[cfg(test)]
use attributes::timestamp_postcondition_evidence;
use effects::classified_error;
#[cfg(test)]
use effects::classify_failure;
use effects::classify_query_error;
use effects::decide_effect;
use effects::decide_generation_effect;
use effects::decide_resize_effect;
use effects::decide_write_effect;
use effects::error_proves_no_effect;
use effects::invalidates_generation;
use effects::mutation_failure_evidence;
#[cfg(test)]
pub(crate) use handles::close;
use handles::execute_close;
pub(crate) use initial_files::is_immutable_initial_file;
use initial_files::load_error;
pub(crate) use initial_files::prepare_initial_files;
pub(crate) use initial_files::provision_initial_files;
pub(crate) use initial_files::update_initial_files;
use namespace::coordinated_target;
pub(crate) use namespace::edit_namespace;
#[cfg(test)]
use namespace::hard_link_postcondition_evidence;
#[cfg(test)]
use namespace::insert_postcondition_evidence;
#[cfg(test)]
use namespace::move_postcondition_evidence;
use namespace::refuse_read_only_change;
#[cfg(test)]
use namespace::remove_postcondition_evidence;
#[cfg(test)]
use namespace::resolve_path_target;
use operations::agent_object_kind;
pub(crate) use operations::attributes;
use operations::execute_open;
pub(crate) use operations::flush;
pub(crate) use operations::is_same_object;
pub(crate) use operations::list_directory;
pub(crate) use operations::open;
#[cfg(test)]
use operations::open_namespace_coordination;
pub(crate) use operations::read_file;
use operations::sandbox_object_kind;
pub(crate) use operations::symlink_target;
#[cfg(test)]
use operations::unwritten_write_suffix;
pub(crate) use operations::write;
pub(crate) use registry::filesystem_activity;
#[cfg(test)]
pub(crate) use registry::test_tree_marks;
pub(crate) use transitions::abort_reconstruction;
pub(crate) use transitions::bind_configured_resource_usage_metering;
#[cfg(test)]
pub(crate) use transitions::bind_resource_usage_metering;
#[cfg(test)]
pub(crate) use transitions::create_fresh;
pub(super) use transitions::create_fresh_with_pressure_recovery;
#[cfg(test)]
use transitions::create_fresh_with_recovery;
#[cfg(test)]
use transitions::decide_limit_transition;
pub(crate) use transitions::delete;
pub(crate) use transitions::delete_created;
pub(crate) use transitions::drain_sealed_filesystem;
pub(crate) use transitions::finish_reconstruction;
pub(crate) use transitions::finish_replay;
pub(crate) use transitions::open_resource_usage_window;
pub(crate) use transitions::reconstruction_generation_handle;
pub(crate) use transitions::resident_generation_handle;
pub(crate) use transitions::seal;
use transitions::sealed_filesystem;
pub(crate) use transitions::set_limits;
use transitions::spawn_module_task;

const WRITE_PRESSURE_RECOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

mod baseline;

mod initial_files;

pub(crate) use baseline::{
    CaptureError, CaptureOutcome, ChangeDetection, FilesystemCapture, InitialFilesCheck,
    InitialFilesRestore, RestoreClass, RestoreError, RestoreTree, WholeCapture, capture,
    capture_whole, check_initial_files, materialize_baseline, tree_mark,
};

pub(crate) use initial_files::InitialFileConflict;
use initial_files::InitialFileState;

mod inspection;

pub(crate) use inspection::{FileInspection, open_file_for_inspection};

mod inspection_stream;

pub(crate) use inspection_stream::produce_file_read;

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

/// The initial-file declarations of a component revision, with their verified content sources.
pub(crate) struct PreparedInitialFiles {
    files: Vec<PreparedInitialFile>,
    loader: Arc<FileLoader>,
    environment_id: EnvironmentId,
}

struct PreparedInitialFile {
    source: InitialFileSource,
    target: PathBuf,
    initial_file: InitialAgentFile,
}

struct FilesystemGeneration<Adapter: SandboxFilesystemAdapter> {
    usage_activation: Option<crate::services::resource_usage_metering::ResourceUsageLiveActivation>,
    sandbox: tokio::sync::RwLock<Option<Arc<Adapter>>>,
    allocation_reader: Adapter::AllocationReader,
    registry: Arc<GenerationRegistry>,
    limits: Mutex<ResolvedStorageLimits>,
    initial_files: Mutex<Arc<InitialFileState>>,
    initial_file_updates: tokio::sync::Mutex<()>,
    pressure_recovery: Option<FilesystemWriteRecovery>,
    accounting: AgentAccounting,
    namespace: Arc<NamespaceCoordinator>,
    release_metrics: std::sync::OnceLock<crate::metrics::resource_release::FilesystemRelease>,
    scratch: Arc<HostDirectory>,
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
    invocation: Option<FilesystemInvocation>,
}

/// Tracks backend work, including calls and descriptor closes whose observer was dropped.
#[derive(Clone)]
pub(crate) struct FilesystemInvocation {
    activity: Arc<InvocationActivity>,
}

struct InvocationActivity {
    state: Mutex<InvocationActivityState>,
    changed: tokio::sync::Notify,
}

struct InvocationActivityState {
    closed: bool,
    active: usize,
}

struct InvocationLease(FilesystemInvocation);

#[derive(Clone, Debug)]
pub(crate) struct ResidentFilesystemActivity {
    registry: Weak<GenerationRegistry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationHandlePhase {
    Resident,
    Reconstruction,
}

impl<Stage: FilesystemStage, Adapter: SandboxFilesystemAdapter> AgentFilesystem<Stage, Adapter> {
    #[cfg(test)]
    pub(crate) fn set_release_metrics_for_test(
        &self,
        local: crate::metrics::resource_release::ReleaseScope,
    ) {
        assert!(
            self.generation
                .as_ref()
                .unwrap()
                .release_metrics
                .set(crate::metrics::resource_release::FilesystemRelease::new(
                    local
                ))
                .is_ok()
        );
    }

    pub(crate) fn observe_release(&self, scope: &crate::metrics::resource_release::ReleaseScope) {
        self.generation
            .as_ref()
            .unwrap()
            .release_metrics
            .get_or_init(Default::default)
            .attach(scope);
    }

    pub(crate) fn accept_deletion_observation(&self) {
        self.generation
            .as_ref()
            .unwrap()
            .release_metrics
            .get_or_init(Default::default)
            .accept();
    }

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

#[derive(Clone)]
pub(crate) struct DeleteFailure {
    pub(crate) source: Arc<FilesystemStorageError>,
    retry: Option<RetryDeletion>,
}

type RetryDeletion = Arc<dyn Fn() -> BoxFuture<'static, Result<(), DeleteFailure>> + Send + Sync>;

impl Debug for DeleteFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeleteFailure")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl DeleteFailure {
    fn unverified(operation: &'static str) -> Self {
        Self {
            source: Arc::new(FilesystemStorageError::verification(operation, None)),
            retry: None,
        }
    }

    pub(crate) fn retry(&self) -> BoxFuture<'static, Result<(), DeleteFailure>> {
        match &self.retry {
            Some(retry) => retry(),
            None => {
                let failure = self.clone();
                Box::pin(async move { Err(failure) })
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Access(AccessError),
    Sandbox(FilesystemStorageError),
    /// The blob storage could not give an initial-file source that an install or a start
    /// loads. The load changes nothing, and a later load can pass.
    InitialFileUnavailable(FilesystemStorageError),
    /// The blob storage refuses the executor a read of an initial-file source that an install or
    /// a start loads. The load changes nothing, and a later load passes only after an operator
    /// fixes the storage.
    InitialFileRefused {
        refusal: BlobRefusal,
        source: FilesystemStorageError,
    },
    /// An install of initial files found something other than what the old declarations left at a
    /// path. The install changed nothing, and the generation stays valid.
    InitialFileConflict(Box<InitialFileConflict>),
    AgentQuota(FilesystemStorageError),
    PhysicalCapacity(FilesystemStorageError),
    Baseline(Box<RestoreError>),
    RuntimeInvalidated,
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Access(error) => Display::fmt(error, formatter),
            Self::Sandbox(error)
            | Self::InitialFileUnavailable(error)
            | Self::InitialFileRefused { source: error, .. }
            | Self::AgentQuota(error)
            | Self::PhysicalCapacity(error) => Display::fmt(error, formatter),
            Self::InitialFileConflict(error) => Display::fmt(error, formatter),
            Self::Baseline(error) => Display::fmt(error, formatter),
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

impl From<FilesystemOperationError> for Error {
    fn from(source: FilesystemOperationError) -> Self {
        Self::Sandbox(source.into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessError {
    Revoked,
    Transitioning,
    WrongGeneration,
    ReadOnly,
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
            Self::ReadOnly => formatter.write_str("filesystem directory lacks mutation capability"),
            Self::NotPermitted => formatter.write_str("filesystem target is read-only"),
        }
    }
}

impl std::error::Error for AccessError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LimitDecision {
    Resident,
    MustUnload,
}

struct GenerationUsageReader<Reader> {
    reader: Reader,
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
            FilesystemStorageError::verification("observe deleted agent filesystem", None)
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

/// What an entry point of the lifecycle can do to the tree. Each entry point gives it when it
/// takes its call lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallEffect {
    /// The call does not change the tree.
    Read,
    /// The call can change the tree, and the kernel gives each object that it changes the time of
    /// the change.
    Changes,
    /// The call can put an existing or a chosen modification time at a path: a rename or a hard
    /// link.
    ChoosesTimes,
    /// The call installs initial files. The seeds copy the modification times of the cached
    /// files.
    Installs,
    /// The call decides after a check if it changes the tree, and records the change itself with
    /// [`GenerationRegistry::record`] before its first change.
    Decides,
}

/// A change that the counters of a generation count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Counted {
    /// The kernel gives each changed object the time of the change.
    Fresh,
    /// An install of initial files. The seeds copy the modification times of the cached files.
    Installed,
    /// The change puts an existing or a chosen modification time at a path outside an install.
    Chosen,
    /// The times that a stat recorded are put back at a path, live or in a replay. A start from
    /// initial files does not hold such a time, but the agent did not choose it either: a replay
    /// of the same history puts it back again, so it does not count as a chosen time.
    Replayed,
}

/// Who asks for a change of the times of an object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimeOrigin {
    /// A call of the agent.
    Call,
    /// The times that a stat recorded, put back live or in a replay.
    Replay,
}

/// The modification times that a restored baseline holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoredTimes {
    /// The times of a save. `kept` tells whether the tree keeps them, which is false when the
    /// initial-file rule changes the tree.
    Saved { kept: bool },
    /// The times of an install of initial files.
    Installed,
}

impl RestoredTimes {
    /// The times of a restored tree that holds the times of a save when `saved_times` is true,
    /// and whose declarations did not change at the restore when `unchanged` is true.
    fn of(saved_times: bool, unchanged: bool) -> Self {
        if saved_times {
            Self::Saved { kept: unchanged }
        } else {
            Self::Installed
        }
    }
}

/// The counters of the tree of one generation. They only grow.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TreeCounters {
    /// The number of changes that could change the tree.
    changes: u64,
    /// The number of those changes that could put an existing or a chosen modification time at a
    /// path.
    carried: u64,
    /// The number of changes that could put a chosen modification time at a path outside an
    /// install of initial files: a rename, a hard link, a set of a given time by a call of the
    /// agent, and a restore of the times of a save. A time that a stat recorded and that is put
    /// back does not count.
    chosen: u64,
    /// Whether the baseline is a restored tree with the modification times of its save.
    restored_times: bool,
}

/// The state of the tree of one generation at one moment, as the counters of the lifecycle give
/// it.
///
/// The counters only grow, and each generation has its own number, so a mark never matches a
/// later tree that differs from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TreeMark {
    generation: u64,
    changes: u64,
    carried: u64,
    /// Whether the tree at this mark has the modification times that a save of the tree keeps. A
    /// capture gives such a mark. A baseline gives one only when it is a restored tree that the
    /// initial-file rule did not change.
    saved_times: bool,
}

/// The source of the generation numbers of [`TreeMark`].
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct RegistryState {
    admission: AdmissionState,
    replay_access: bool,
    calls: usize,
    nodes: usize,
    /// The number of the generation, unique in the process.
    generation: u64,
    counters: TreeCounters,
}

struct CallLease {
    registry: Arc<GenerationRegistry>,
    invocation: Option<InvocationLease>,
}

struct NodeLease {
    registry: Arc<GenerationRegistry>,
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
    namespace: Vec<SandboxNameComparisonKey>,
    directories: Vec<SandboxDirectoryId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NamespaceCoordinationKind {
    Observe,
    Edit,
}

struct NamespaceCoordination {
    coordinator: Arc<NamespaceCoordinator>,
    id: u64,
    active: bool,
}

type CallTask<T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'static>>;

#[must_use]
pub(crate) struct FilesystemCall<T: 'static> {
    state: CallState<T>,
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

#[must_use]
pub(crate) struct FilesystemClose {
    state: CloseState,
}

enum CloseState {
    Unstarted { task: Option<CallTask<()>> },
    CallerDriven { task: Option<CallTask<()>> },
    Done,
}

pub(crate) enum OpenNode {
    File(File),
    Directory(Directory),
}

pub(crate) struct File {
    ownership: NodeOwnership,
}

pub(crate) struct Directory {
    ownership: NodeOwnership,
}

struct NodeOwnership {
    sandbox: Option<SandboxHandle>,
    generation_id: usize,
    access: AccessMode,
    node_lease: Option<NodeLease>,
    invocation: Option<InvocationLease>,
    close: Option<Box<dyn FnOnce(SandboxHandle, NodeLease) -> FilesystemClose + Send>>,
}

impl NodeOwnership {
    fn sandbox(&self) -> &SandboxHandle {
        self.sandbox
            .as_ref()
            .expect("filesystem node already closed")
    }

    fn into_close(mut self) -> FilesystemClose {
        let sandbox = self.sandbox.take().expect("filesystem node already closed");
        let node_lease = self
            .node_lease
            .take()
            .expect("filesystem node lease missing");
        self.close.take().expect("filesystem node close missing")(sandbox, node_lease)
            .retain_invocation(self.invocation.take())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PathTarget {
    sandbox: SandboxPath,
    generation_id: usize,
    access: AccessMode,
}

impl PathTarget {
    pub(crate) fn require_mutable(&self) -> Result<(), AccessError> {
        if self.access.can_write() {
            Ok(())
        } else {
            Err(AccessError::ReadOnly)
        }
    }

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
        let SandboxHandle::Directory(directory) = directory.ownership.sandbox() else {
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
    Open(SandboxHandle),
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
pub(crate) enum FlushLevel {
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
    None,
    Read,
    /// Reads, and changes the times of the object that the open pinned. A guest open never asks
    /// for this. The lifecycle uses it for a followed attribute change, where the change must
    /// land on the object that the read-only check was decided against.
    ReadAndSetTimes,
    Write,
    ReadWrite,
}

impl AccessMode {
    pub(crate) fn can_read(self) -> bool {
        matches!(self, Self::Read | Self::ReadAndSetTimes | Self::ReadWrite)
    }

    pub(crate) fn can_write(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }

    fn can_set_times(self) -> bool {
        self.can_write() || self == Self::ReadAndSetTimes
    }
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
    /// Whether the object has no write permission.
    pub(crate) read_only: bool,
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

/// A namespace entry under coordination, with a resolution of the entry.
struct CoordinatedTarget {
    coordination: NamespaceCoordination,
    resolved: SandboxResolvedPathTarget,
}

enum ResolvedNamespaceEdit {
    Insert {
        destination: SandboxResolvedPathTarget,
        object: NewObject,
    },
    Link {
        source: SandboxResolvedPathTarget,
        destination: SandboxResolvedPathTarget,
    },
    Move {
        source: SandboxResolvedPathTarget,
        destination: SandboxResolvedPathTarget,
    },
    Remove {
        target: SandboxResolvedPathTarget,
        expected: ObjectKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InsertPostcondition {
    path: NamespacePathState,
    desired: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NamespacePathState {
    Absent,
    Present(SandboxAttributes),
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

fn sandbox_follow(follow: Follow) -> SandboxFollow {
    match follow {
        Follow::Yes => SandboxFollow::Yes,
        Follow::No => SandboxFollow::No,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FailureFacts {
    quota_exhausted: bool,
    physical_capacity_exhausted: bool,
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(all(test, target_os = "linux"))]
mod workload_benchmark;
