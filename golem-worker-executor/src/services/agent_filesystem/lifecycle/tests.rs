use super::*;
use crate::filesystem_pressure::FilesystemWriteRecoveryAuthority;
use crate::sandbox_filesystem::{
    FilesystemAllocation, ScriptedSandboxFilesystemControl, ScriptedSandboxFilesystemProvisioning,
    ScriptedSandboxPath, ScriptedSandboxPathBase, ScriptedSandboxPathCall,
};
use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
use crate::services::golem_config::{FilesystemStorageConfig, ResourceUsageMeteringConfig};
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::close_window;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::{AgentFilePath, AgentFilePermissions, ComponentId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use golem_common::widen_infallible;
use golem_service_base::replayable_stream::ReplayableStream as _;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use proptest::prelude::*;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use tokio::sync::Notify;
use uuid::Uuid;

struct ScriptedWriteRecovery {
    outcomes: Mutex<VecDeque<FilesystemWriteRecoveryOutcome>>,
    calls: AtomicUsize,
}

impl ScriptedWriteRecovery {
    fn new(outcomes: impl IntoIterator<Item = FilesystemWriteRecoveryOutcome>) -> Arc<Self> {
        Arc::new(Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            calls: AtomicUsize::new(0),
        })
    }

    fn handle(self: &Arc<Self>) -> FilesystemWriteRecovery {
        FilesystemWriteRecovery::scripted(self.clone())
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl FilesystemWriteRecoveryAuthority for ScriptedWriteRecovery {
    async fn recover_write(&self, _deadline: Instant) -> FilesystemWriteRecoveryOutcome {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.outcomes.lock().unwrap().pop_front().unwrap()
    }
}

struct BlockingWriteRecovery {
    started: Notify,
    release: Notify,
    calls: AtomicUsize,
}

impl BlockingWriteRecovery {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
        })
    }

    fn handle(self: &Arc<Self>) -> FilesystemWriteRecovery {
        FilesystemWriteRecovery::scripted(self.clone())
    }

    async fn wait_started(&self) {
        self.started.notified().await;
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

#[async_trait::async_trait]
impl FilesystemWriteRecoveryAuthority for BlockingWriteRecovery {
    async fn recover_write(&self, _deadline: Instant) -> FilesystemWriteRecoveryOutcome {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.started.notify_one();
        self.release.notified().await;
        FilesystemWriteRecoveryOutcome::Recovered
    }
}

fn agent_id() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId::from_agent_name_string(ComponentId::new(), "lifecycle").unwrap(),
    )
}

fn limits(bytes: u64, objects: u64) -> FilesystemLimits {
    FilesystemLimits {
        allocated_bytes: bytes,
        filesystem_objects: objects,
    }
}

fn allocation(bytes: u64, objects: u64) -> FilesystemAllocation {
    FilesystemAllocation {
        allocated_bytes: bytes,
        filesystem_objects: objects,
    }
}

fn unsupported_allocation() -> FilesystemStorageError {
    FilesystemStorageError::verification(
        "observe allocation without quota authority",
        Path::new("<scripted>"),
    )
}

fn sandbox_error(operation: &'static str, kind: std::io::ErrorKind) -> FilesystemStorageError {
    FilesystemStorageError::io(
        operation,
        Path::new("<scripted>"),
        std::io::Error::new(kind, operation),
    )
}

fn sandbox_provisioning(
    settings: &FilesystemStorageConfig,
) -> Result<SandboxFilesystemProvisioning, FilesystemStorageError> {
    SandboxFilesystemProvisioning::new(
        settings.deterministic_root_dir.clone(),
        settings.managed_xfs_root_dir.clone(),
        settings.cleanup_retry.clone(),
    )
}

async fn prepared_initial_file() -> (PreparedInitialFiles, Arc<FileLoader>) {
    let id = agent_id();
    let service = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let content = b"prepared initial file".to_vec();
    let content_hash = service
        .put_if_not_exists(
            id.environment_id,
            content
                .clone()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await
        .unwrap();
    let loader = Arc::new(FileLoader::new(service, None).unwrap());
    let prepared = prepare_initial_files(
        &loader,
        id.environment_id,
        &[InitialAgentFile {
            content_hash,
            path: AgentFilePath::from_abs_str("/prepared").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            size: content.len() as u64,
        }],
    )
    .await
    .unwrap();
    (prepared, loader)
}

fn sandbox_attributes(kind: SandboxObjectKind) -> SandboxAttributes {
    SandboxAttributes {
        kind,
        link_count: 1,
        size: 0,
        accessed: None,
        modified: None,
    }
}

fn missing(operation: &'static str) -> FilesystemStorageError {
    sandbox_error(operation, std::io::ErrorKind::NotFound)
}

fn account() -> (ResourceUsageAccount, Arc<AtomicResourceEntry>) {
    configured_account(true)
}

fn configured_account(memory_metering: bool) -> (ResourceUsageAccount, Arc<AtomicResourceEntry>) {
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let memory = LinearMemoryTracker::new_with_metering(
        0,
        0,
        AgentMode::Durable,
        false,
        entry.clone(),
        Arc::new(Mutex::new(MemoryGrant::inert(0))),
        memory_metering,
    );
    (
        ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone()),
        entry,
    )
}

async fn permit(entry: &Arc<AtomicResourceEntry>) -> ConcurrentAgentPermit {
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account_id = AccountId(Uuid::new_v4());
    scheduler.register_account(account_id, entry.clone()).await;
    scheduler.acquire(account_id, agent_id().agent_id).await
}

async fn reconstructing(
    limits: ResolvedStorageLimits,
) -> (
    TestAgentFilesystem<Reconstructing>,
    ScriptedSandboxFilesystemControl,
    Arc<AtomicResourceEntry>,
) {
    reconstructing_with_recovery(limits, None).await
}

async fn reconstructing_with_recovery(
    limits: ResolvedStorageLimits,
    pressure_recovery: Option<FilesystemWriteRecovery>,
) -> (
    TestAgentFilesystem<Reconstructing>,
    ScriptedSandboxFilesystemControl,
    Arc<AtomicResourceEntry>,
) {
    let (filesystem, control, entry) =
        bound_reconstructing_with_recovery(limits, pressure_recovery).await;
    let filesystem = materialize_initial_files(filesystem, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let filesystem = finish_replay(filesystem).await.unwrap();
    (filesystem, control, entry)
}

async fn bound_reconstructing_with_recovery(
    limits: ResolvedStorageLimits,
    pressure_recovery: Option<FilesystemWriteRecovery>,
) -> (
    TestAgentFilesystem<Reconstructing>,
    ScriptedSandboxFilesystemControl,
    Arc<AtomicResourceEntry>,
) {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let created = create_fresh_with_recovery::<ScriptedSandboxFilesystem>(
        provisioning,
        agent_id(),
        limits,
        pressure_recovery,
    )
    .await
    .unwrap();
    let (account, entry) = account();
    let filesystem = bind_configured_resource_usage_metering(
        created,
        account,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        },
    )
    .unwrap();
    (filesystem, control, entry)
}

async fn unmetered_reconstructing_with_recovery(
    limits: ResolvedStorageLimits,
    pressure_recovery: Option<FilesystemWriteRecovery>,
) -> (
    TestAgentFilesystem<Reconstructing>,
    ScriptedSandboxFilesystemControl,
    Arc<AtomicResourceEntry>,
) {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let created = create_fresh_with_recovery::<ScriptedSandboxFilesystem>(
        provisioning,
        agent_id(),
        limits,
        pressure_recovery,
    )
    .await
    .unwrap();
    let (account, entry) = configured_account(false);
    let filesystem = bind_configured_resource_usage_metering(
        created,
        account,
        ResourceUsageMeteringConfig::default(),
    )
    .unwrap();
    (filesystem, control, entry)
}

#[test]
async fn created_product_supports_observed_verified_cleanup() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let created = create_fresh_with_recovery::<ScriptedSandboxFilesystem>(
        provisioning,
        agent_id(),
        ResolvedStorageLimits::Unlimited,
        None,
    )
    .await
    .unwrap();
    control.push_delete_and_verify(Ok(()));

    delete_created(created).await.unwrap();

    assert!(has_call(&control, "delete_and_verify("));
}

async fn resident(
    usage: Result<FilesystemAllocation, FilesystemStorageError>,
) -> (
    TestAgentFilesystem<Resident>,
    ScriptedSandboxFilesystemControl,
    Arc<AtomicResourceEntry>,
) {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(usage);
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    (filesystem, control, entry)
}

async fn metered_resident() -> (
    TestAgentFilesystem<Resident>,
    ScriptedSandboxFilesystemControl,
    ResourceUsageMeteringWindow,
) {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    (filesystem, control, window)
}

pub(crate) async fn resident_for_unload_test() -> (
    ResidentFilesystem<ScriptedSandboxFilesystem>,
    ScriptedSandboxFilesystemControl,
) {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    (filesystem, control)
}

pub(crate) async fn metered_resident_with_open_node_for_unload_test() -> (
    ResidentFilesystem<ScriptedSandboxFilesystem>,
    ScriptedSandboxFilesystemControl,
    ResourceUsageMeteringWindow,
    FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    OpenNode,
) {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 10_001).await);
    (filesystem, control, window, generation_handle, node)
}

pub(crate) async fn billing_metered_resident_with_open_node_for_unload_test() -> (
    ResidentFilesystem<ScriptedSandboxFilesystem>,
    ScriptedSandboxFilesystemControl,
    ResourceUsageMeteringWindow,
    FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    OpenNode,
) {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let created = create_fresh_with_recovery::<ScriptedSandboxFilesystem>(
        provisioning,
        agent_id(),
        ResolvedStorageLimits::Unlimited,
        None,
    )
    .await
    .unwrap();
    let (account, entry) = account();
    let reconstructing = bind_resource_usage_metering(created, account).unwrap();
    let reconstructing = materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstructing = finish_replay(reconstructing).await.unwrap();
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(reconstructing).await.unwrap();
    control.push_observe_allocation(Ok(FilesystemAllocation {
        allocated_bytes: 50,
        filesystem_objects: 1,
    }));
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count()
            < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 10_002).await);
    (filesystem, control, window, generation_handle, node)
}

async fn authoritative_metered_resident(
    limits: ResolvedStorageLimits,
    baseline: FilesystemAllocation,
) -> (
    TestAgentFilesystem<Resident>,
    ScriptedSandboxFilesystemControl,
    ResourceUsageMeteringWindow,
) {
    authoritative_metered_resident_with_recovery(limits, baseline, None).await
}

async fn authoritative_metered_resident_with_recovery(
    limits: ResolvedStorageLimits,
    baseline: FilesystemAllocation,
    pressure_recovery: Option<FilesystemWriteRecovery>,
) -> (
    TestAgentFilesystem<Resident>,
    ScriptedSandboxFilesystemControl,
    ResourceUsageMeteringWindow,
) {
    let (filesystem, control, entry) =
        reconstructing_with_recovery(limits, pressure_recovery).await;
    control.push_observe_allocation(Ok(baseline));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    (filesystem, control, window)
}

async fn open_file(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    id: u64,
) -> File {
    open_file_with_access(generation_handle, control, id, AccessMode::ReadWrite).await
}

async fn open_file_with_access(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    id: u64,
    mode: AccessMode,
) -> File {
    control.push_open(Ok(SandboxOpened::scripted_file(id)));
    open_programmed_file(generation_handle, mode, "file").await
}

async fn open_programmed_file(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    mode: AccessMode,
    path: &str,
) -> File {
    let opened = open(
        generation_handle,
        PathTarget::at_root(generation_handle, path).unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: mode,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    match opened.node {
        OpenNode::File(file) => file,
        OpenNode::Directory(_) => panic!("scripted open returned a directory"),
    }
}

async fn open_directory(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    id: u64,
) -> Directory {
    open_directory_with_access(generation_handle, control, id, AccessMode::ReadWrite).await
}

async fn open_directory_with_access(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    id: u64,
    mode: AccessMode,
) -> Directory {
    open_directory_at(generation_handle, control, id, mode, "directory").await
}

async fn open_directory_at(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    id: u64,
    mode: AccessMode,
    path: &str,
) -> Directory {
    open_directory_target(
        generation_handle,
        control,
        id,
        mode,
        PathTarget::at_root(generation_handle, path).unwrap(),
    )
    .await
}

async fn open_directory_with_identity(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    handle: u64,
    identity: u64,
    path: &str,
) -> Directory {
    control.push_open(Ok(SandboxOpened::scripted_directory_with_identity(
        handle, identity,
    )));
    let opened = open(
        generation_handle,
        PathTarget::at_root(generation_handle, path).unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::ReadWrite,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    match opened.node {
        OpenNode::Directory(directory) => directory,
        OpenNode::File(_) => panic!("scripted open returned a file"),
    }
}

async fn open_directory_target(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    id: u64,
    mode: AccessMode,
    target: PathTarget,
) -> Directory {
    control.push_open(Ok(SandboxOpened::scripted_directory(id)));
    let opened = open(
        generation_handle,
        target,
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: mode,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    match opened.node {
        OpenNode::Directory(directory) => directory,
        OpenNode::File(_) => panic!("scripted open returned a file"),
    }
}

fn has_call(control: &ScriptedSandboxFilesystemControl, operation: &str) -> bool {
    control
        .calls()
        .iter()
        .any(|call| call.starts_with(operation))
}

fn call_count(control: &ScriptedSandboxFilesystemControl, operation: &str) -> usize {
    control
        .calls()
        .iter()
        .filter(|call| call.starts_with(operation))
        .count()
}

async fn wait_for_call(control: &ScriptedSandboxFilesystemControl, operation: &str) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !has_call(control, operation) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn assert_insert_coordination(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    first: PathTarget,
    second: PathTarget,
    should_serialize: bool,
) {
    control.push_get_attributes(Err(missing("first coordinated insert before")));
    control.push_get_attributes(Err(missing("second coordinated insert before")));
    control.push_create_directory(Ok(()));
    control.push_create_directory(Ok(()));
    let gate = control.block("create_directory");
    let first = tokio::spawn(
        edit_namespace(
            generation_handle,
            NamespaceEdit::Insert {
                destination: first,
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    gate.wait_started().await;
    let second = tokio::spawn(
        edit_namespace(
            generation_handle,
            NamespaceEdit::Insert {
                destination: second,
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );

    if should_serialize {
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !second.is_finished(),
            "conflicting namespace edits overlapped"
        );
        gate.release();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    } else {
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("independent namespace edit was serialized")
            .unwrap()
            .unwrap();
        assert!(!first.is_finished());
        gate.release();
        first.await.unwrap().unwrap();
    }
}

async fn move_namespace_entry(
    generation_handle: &FilesystemGenerationHandle<ScriptedSandboxFilesystem>,
    control: &ScriptedSandboxFilesystemControl,
    source: PathTarget,
    destination: PathTarget,
    source_kind: SandboxObjectKind,
) {
    control.push_get_attributes(Ok(sandbox_attributes(source_kind)));
    control.push_rename(Ok(()));
    edit_namespace(
        generation_handle,
        NamespaceEdit::Move {
            source,
            destination,
        },
    )
    .unwrap()
    .await
    .unwrap();
}

#[test]
async fn stages_share_one_generation_and_only_resident_generation_handle_is_published() {
    let (filesystem, control, _) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    let generation = filesystem.generation.as_ref().unwrap().clone();
    let generation_id = Arc::as_ptr(&generation);
    drop(generation);
    control.push_observe_allocation(Err(unsupported_allocation()));

    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    assert_eq!(
        Arc::as_ptr(&generation_handle.generation.upgrade().unwrap()),
        generation_id
    );
    let filesystem = seal(filesystem);
    assert_eq!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "rejected").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        )
        .unwrap_err(),
        AccessError::Revoked
    );

    control.push_delete_and_verify(Ok(()));
    delete(filesystem).await.unwrap();
    assert!(generation_handle.generation.upgrade().is_none());
}

#[test]
async fn reconstruction_requires_initial_materialization_and_replay_drain() {
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let failure = finish_reconstruction(filesystem).await.unwrap_err();
    assert!(matches!(failure.source, Error::RuntimeInvalidated));
    assert!(!has_call(&control, "observe_allocation("));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();

    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let filesystem = materialize_initial_files(filesystem, PreparedInitialFiles::empty())
        .await
        .unwrap();
    assert!(matches!(
        reconstruction_generation_handle(&filesystem),
        Ok(FilesystemGenerationHandle {
            phase: GenerationHandlePhase::Reconstruction,
            ..
        })
    ));
    let failure = finish_reconstruction(filesystem).await.unwrap_err();
    assert!(matches!(failure.source, Error::RuntimeInvalidated));
    assert!(!has_call(&control, "observe_allocation("));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn initial_file_materialization_without_storage_metering_needs_no_billing_window() {
    let id = agent_id();
    let service = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let content = b"metered initial file".to_vec();
    let content_hash = service
        .put_if_not_exists(
            id.environment_id,
            content
                .clone()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await
        .unwrap();
    let loader = Arc::new(FileLoader::new(service, None).unwrap());
    let prepared = prepare_initial_files(
        &loader,
        id.environment_id,
        &[InitialAgentFile {
            content_hash,
            path: AgentFilePath::from_abs_str("/metered").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            size: content.len() as u64,
        }],
    )
    .await
    .unwrap();
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed_file(Ok(()));

    let filesystem = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap();
    assert!(has_call(&control, "seed_file("));
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
    drop(loader);
}

#[test]
#[timeout("5s")]
async fn external_seed_source_retains_its_cache_lease_through_seeding() {
    let (prepared, loader) = prepared_initial_file().await;
    let source_path = prepared.files[0].source.path().to_path_buf();
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    control.push_seed_file(Ok(()));
    let gate = control.block("seed_file");

    let materializing = tokio::spawn(materialize_initial_files(filesystem, prepared));
    gate.wait_started().await;
    assert!(source_path.exists());
    let seed_call = control
        .calls()
        .into_iter()
        .find(|call| call.starts_with("seed_file("))
        .unwrap();
    assert!(seed_call.contains(&format!("source={}", source_path.display())));

    gate.release();
    let filesystem = materializing.await.unwrap().unwrap();
    assert!(!source_path.exists());
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
    drop(loader);
}

#[test]
async fn reconstruction_seed_storage_full_at_limit_is_agent_quota() {
    let storage_limits = limits(4096, 8);
    let (filesystem, control, entry) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Finite(storage_limits), None)
            .await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Err(sandbox_error(
        "seed initial file",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_observe_allocation(Ok(allocation(storage_limits.allocated_bytes, 1)));

    let (prepared, loader) = prepared_initial_file().await;
    let failure = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap_err();

    assert!(matches!(failure.source, Error::AgentQuota(_)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
    drop(loader);
}

#[test]
async fn managed_quota_behavior_remains_active_with_storage_metering_disabled() {
    let storage_limits = limits(4096, 8);
    let (filesystem, control, entry) =
        unmetered_reconstructing_with_recovery(ResolvedStorageLimits::Finite(storage_limits), None)
            .await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Err(sandbox_error(
        "seed initial file",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_observe_allocation(Ok(allocation(storage_limits.allocated_bytes, 1)));
    let (prepared, loader) = prepared_initial_file().await;

    let failure = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap_err();

    assert!(matches!(failure.source, Error::AgentQuota(_)));
    assert_eq!(call_count(&control, "observe_allocation("), 1);
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(call_count(&control, "observe_allocation("), 1);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
    drop(loader);
}

#[test]
async fn managed_pressure_behavior_remains_active_with_storage_metering_disabled() {
    let storage_limits = limits(4096, 8);
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Denied]);
    let (filesystem, control, entry) = unmetered_reconstructing_with_recovery(
        ResolvedStorageLimits::Finite(storage_limits),
        Some(recovery.handle()),
    )
    .await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Err(sandbox_error(
        "seed initial file",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_observe_allocation(Ok(allocation(512, 1)));
    let (prepared, loader) = prepared_initial_file().await;

    let failure = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap_err();

    assert!(matches!(failure.source, Error::PhysicalCapacity(_)));
    assert_eq!(recovery.calls(), 1);
    assert_eq!(call_count(&control, "observe_allocation("), 1);
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(call_count(&control, "observe_allocation("), 1);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
    drop(loader);
}

#[test]
async fn reconstruction_seed_storage_full_below_limit_is_physical_capacity() {
    let storage_limits = limits(4096, 8);
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Denied]);
    let (filesystem, control, entry) = bound_reconstructing_with_recovery(
        ResolvedStorageLimits::Finite(storage_limits),
        Some(recovery.handle()),
    )
    .await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Err(sandbox_error(
        "seed initial file",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_observe_allocation(Ok(allocation(512, 1)));

    let (prepared, loader) = prepared_initial_file().await;
    let failure = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap_err();

    assert!(matches!(failure.source, Error::PhysicalCapacity(_)));
    assert_eq!(recovery.calls(), 1);
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
    drop(loader);
}

#[test]
async fn reconstruction_seed_retries_after_physical_capacity_recovery() {
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, entry) = bound_reconstructing_with_recovery(
        ResolvedStorageLimits::Finite(limits(4096, 8)),
        Some(recovery.handle()),
    )
    .await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Err(sandbox_error(
        "seed initial file",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_observe_allocation(Ok(allocation(512, 1)));
    control.push_seed_file(Ok(()));
    control.push_observe_allocation(Ok(allocation(1024, 1)));
    let (prepared, loader) = prepared_initial_file().await;

    let filesystem = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap();

    assert_eq!(recovery.calls(), 1);
    assert_eq!(call_count(&control, "seed_file("), 2);
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
    drop(loader);
}

#[test]
async fn increased_limit_permits_fresh_reconstruction_after_quota_failure() {
    let low_limits = limits(4096, 8);
    let (filesystem, control, entry) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Finite(low_limits), None).await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Err(sandbox_error(
        "seed initial file",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_observe_allocation(Ok(allocation(low_limits.allocated_bytes, 1)));
    let (prepared, loader) = prepared_initial_file().await;
    let failure = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap_err();
    assert!(matches!(failure.source, Error::AgentQuota(_)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
    drop(loader);

    let (filesystem, control, entry) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Finite(limits(8192, 8)), None)
            .await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    control.push_seed_file(Ok(()));
    control.push_observe_allocation(Ok(allocation(4096, 1)));
    let (prepared, loader) = prepared_initial_file().await;

    let filesystem = materialize_initial_files(filesystem, prepared)
        .await
        .unwrap();

    assert_eq!(call_count(&control, "seed_file("), 1);
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(abort_reconstruction(filesystem)).await.unwrap();
    drop(loader);
}

#[test]
async fn reconstruction_mutations_without_storage_metering_need_no_billing_window() {
    let (filesystem, control, _) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let filesystem = materialize_initial_files(filesystem, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstruction_generation_handle = reconstruction_generation_handle(&filesystem).unwrap();
    let target =
        PathTarget::at_root(&reconstruction_generation_handle, "replayed-directory").unwrap();

    control.push_get_attributes(Err(missing("replay insert before")));
    control.push_create_directory(Ok(()));

    edit_namespace(
        &reconstruction_generation_handle,
        NamespaceEdit::Insert {
            destination: target,
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert!(has_call(&control, "create_directory("));

    let filesystem = finish_replay(filesystem).await.unwrap();
    assert!(matches!(
        PathTarget::at_root(&reconstruction_generation_handle, "revoked-after-drain"),
        Err(AccessError::Revoked)
    ));
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let resident_generation_handle = resident_generation_handle(&filesystem);
    assert!(matches!(
        resident_generation_handle.phase,
        GenerationHandlePhase::Resident
    ));
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn reconstruction_opened_unlinked_node_release_does_not_observe_allocation() {
    let (filesystem, control, entry) =
        bound_reconstructing_with_recovery(ResolvedStorageLimits::Unlimited, None).await;
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    let filesystem = materialize_initial_files(filesystem, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstruction_generation_handle = reconstruction_generation_handle(&filesystem).unwrap();
    let target = PathTarget::at_root(&reconstruction_generation_handle, "open-unlinked").unwrap();
    control.push_open(Ok(SandboxOpened::scripted_file(90)));
    let node = open(
        &reconstruction_generation_handle,
        target.clone(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::ReadWrite,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap()
    .node;

    let observations_before_unlink = call_count(&control, "observe_allocation(");
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Ok(()));
    edit_namespace(
        &reconstruction_generation_handle,
        NamespaceEdit::Remove {
            target,
            expected: ObjectKind::File,
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before_unlink
    );

    let filesystem = finish_replay(filesystem).await.unwrap();
    assert!(matches!(
        PathTarget::at_root(&reconstruction_generation_handle, "revoked-after-drain"),
        Err(AccessError::Revoked)
    ));
    control.push_observe_allocation(Ok(allocation(90, 1)));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let resident_generation_handle = resident_generation_handle(&filesystem);
    let OpenNode::File(file) = &node else {
        panic!("scripted open returned a directory")
    };
    control.push_read(Ok(Bytes::from_static(b"still-open")));
    assert_eq!(
        read_file(
            &resident_generation_handle,
            file,
            ReadRange {
                offset: 0,
                length: 10,
            },
        )
        .unwrap()
        .await
        .unwrap(),
        Bytes::from_static(b"still-open")
    );

    let observations_before_release = call_count(&control, "observe_allocation(");
    control.push_release(Ok(()));
    let release_gate = control.block("release");
    let releasing = tokio::spawn(release(node));
    release_gate.wait_started().await;
    let closing = tokio::spawn(close_window(
        window,
        Instant::now() + Duration::from_secs(1),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(closing.is_finished());
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before_release
    );

    closing.await.unwrap().unwrap();
    release_gate.release();
    releasing.await.unwrap().unwrap();
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before_release
    );
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn path_capabilities_cannot_cross_generations() {
    let (first, first_control, _) = resident(Err(unsupported_allocation())).await;
    let (second, second_control, _) = resident(Err(unsupported_allocation())).await;
    let first_generation_handle = resident_generation_handle(&first);
    let second_generation_handle = resident_generation_handle(&second);
    let first_target = PathTarget::at_root(&first_generation_handle, "owned-by-first").unwrap();

    assert!(matches!(
        open(
            &second_generation_handle,
            first_target,
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        ),
        Err(AccessError::WrongGeneration)
    ));
    first_control.push_delete_and_verify(Ok(()));
    second_control.push_delete_and_verify(Ok(()));
    delete(seal(first)).await.unwrap();
    delete(seal(second)).await.unwrap();
}

#[test]
async fn abort_reconstruction_returns_a_sealed_deletable_product() {
    let (filesystem, control, _) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    let filesystem = abort_reconstruction(filesystem);
    control.push_delete_and_verify(Ok(()));
    delete(filesystem).await.unwrap();
}

#[test]
async fn failed_reconstruction_returns_the_sealed_failure_product() {
    let (filesystem, control, _) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(sandbox_error(
        "observe allocation",
        std::io::ErrorKind::Other,
    )));

    let failure = finish_reconstruction(filesystem).await.unwrap_err();
    assert!(matches!(failure.source, Error::Sandbox(_)));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropping_reconstruction_observer_never_publishes_resident_and_deletes() {
    let (filesystem, control, _) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    let generation = filesystem.generation.as_ref().unwrap().clone();
    let weak = Arc::downgrade(&generation);
    control.push_observe_allocation(Err(unsupported_allocation()));
    control.push_delete_and_verify(Ok(()));
    let gate = control.block("observe_allocation");

    let transition = finish_reconstruction(filesystem);
    assert!(matches!(
        generation.registry.lease_call(),
        Err(AccessError::Transitioning)
    ));
    drop(transition);
    drop(generation);
    gate.wait_started().await;
    gate.release();

    wait_for_call(&control, "delete_and_verify(").await;
    assert!(weak.upgrade().is_none());
}

#[test]
async fn dropping_an_unstarted_call_cancels_it_without_sandbox_work() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_open(Ok(SandboxOpened::scripted_file(1)));
    let call = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "never-started").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .unwrap();

    drop(call);
    assert!(!has_call(&control, "open("));
    let filesystem = seal(filesystem);
    control.push_delete_and_verify(Ok(()));
    delete(filesystem).await.unwrap();
}

#[test]
async fn first_call_poll_runs_the_exact_operation_in_the_caller_task() {
    let registry = Arc::new(GenerationRegistry::new());
    let lease = registry.lease_call().unwrap();
    let caller_thread = std::thread::current().id();
    let operation_thread = Arc::new(Mutex::new(None));
    let observed_thread = Arc::clone(&operation_thread);
    let call = FilesystemCall::new(lease, async move {
        *observed_thread.lock().unwrap() = Some(std::thread::current().id());
        Ok(17)
    });

    assert_eq!(call.await.unwrap(), 17);
    assert_eq!(*operation_thread.lock().unwrap(), Some(caller_thread));
    assert_eq!(registry.state.lock().unwrap().calls, 0);
}

#[test]
#[timeout("5s")]
async fn dropping_a_started_call_transfers_the_same_operation_once() {
    let registry = Arc::new(GenerationRegistry::new());
    let lease = registry.lease_call().unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let completions = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let call = FilesystemCall::new(lease, {
        let starts = Arc::clone(&starts);
        let completions = Arc::clone(&completions);
        let started = Arc::clone(&started);
        let resume = Arc::clone(&resume);
        async move {
            starts.fetch_add(1, Ordering::AcqRel);
            started.notify_one();
            resume.notified().await;
            completions.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    });

    let observer = tokio::spawn(call);
    started.notified().await;
    observer.abort();
    let _ = observer.await;
    assert_eq!(registry.state.lock().unwrap().calls, 1);

    resume.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while completions.load(Ordering::Acquire) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    registry.wait_for_calls().await;

    assert_eq!(starts.load(Ordering::Acquire), 1);
    assert_eq!(completions.load(Ordering::Acquire), 1);
    assert_eq!(registry.state.lock().unwrap().calls, 0);
}

#[test]
#[timeout("5s")]
async fn dropping_a_started_release_transfers_it_once_and_unblocks_deletion() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 701).await;
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));
    let release_gate = control.block("release");

    let observer = tokio::spawn(release(OpenNode::File(file)));
    release_gate.wait_started().await;
    observer.abort();
    let _ = observer.await;
    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::task::yield_now().await;
    assert!(!deletion.is_finished());

    release_gate.release();
    release_gate.wait_completed().await;
    deletion.await.unwrap().unwrap();
    assert_eq!(call_count(&control, "release("), 1);
}

#[test]
async fn delete_waits_for_started_query_and_release_after_observers_drop() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 7).await;
    control.push_read(Ok(Bytes::from_static(b"read")));
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));
    let read_gate = control.block("read");
    let release_gate = control.block("release");

    let query = tokio::spawn(
        read_file(
            &generation_handle,
            &file,
            ReadRange {
                offset: 0,
                length: 4,
            },
        )
        .unwrap(),
    );
    read_gate.wait_started().await;
    query.abort();
    let _ = query.await;
    drop(release(OpenNode::File(file)));
    release_gate.wait_started().await;

    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::task::yield_now().await;
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));

    read_gate.release();
    tokio::task::yield_now().await;
    assert!(!deletion.is_finished());
    release_gate.release();
    deletion.await.unwrap().unwrap();
}

#[test]
async fn open_finishing_after_seal_registers_then_releases_before_deletion() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_open(Ok(SandboxOpened::scripted_file(9)));
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));
    let open_gate = control.block("open");
    let release_gate = control.block("release");
    let opening = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "late").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        )
        .unwrap(),
    );
    open_gate.wait_started().await;

    let deletion = tokio::spawn(delete(seal(filesystem)));
    open_gate.release();
    release_gate.wait_started().await;
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));

    release_gate.release();
    assert!(matches!(
        opening.await.unwrap(),
        Err(Error::Access(AccessError::Revoked))
    ));
    deletion.await.unwrap().unwrap();
}

#[test]
async fn an_existing_node_can_transition_to_release_after_seal() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 11).await;
    let filesystem = seal(filesystem);
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));

    release(OpenNode::File(file)).await.unwrap();
    delete(filesystem).await.unwrap();
    assert!(has_call(&control, "release("));
    assert!(has_call(&control, "delete_and_verify("));
}

#[test]
async fn delete_waits_for_an_open_node_before_its_release_transition() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 20).await;
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));

    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::task::yield_now().await;
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));
    release(OpenNode::File(file)).await.unwrap();
    deletion.await.unwrap().unwrap();
}

#[test]
async fn delete_barrier_covers_directory_attribute_and_symlink_queries() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 21).await;
    control.push_open(Ok(SandboxOpened::scripted_directory(22)));
    let directory = match open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "directory").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap()
    .node
    {
        OpenNode::Directory(directory) => directory,
        OpenNode::File(_) => panic!("scripted open returned a file"),
    };
    control.push_read_directory(Ok(Vec::new()));
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 4,
        accessed: None,
        modified: None,
    }));
    control.push_read_link(Ok(SandboxSymlinkTarget(PathBuf::from("target"))));
    let directory_gate = control.block("read_directory");
    let attributes_gate = control.block("get_node_attributes");
    let symlink_gate = control.block("read_link");
    let listing = tokio::spawn(list_directory(&generation_handle, &directory).unwrap());
    let attributes = tokio::spawn(
        super::attributes(
            &generation_handle,
            Target::Open(&OpenNode::File(File {
                ownership: NodeOwnership {
                    sandbox: Some(file.ownership.sandbox().clone()),
                    generation_id: file.ownership.generation_id,
                    access: file.ownership.access,
                    node_lease: None,
                    release: None,
                },
            })),
        )
        .unwrap(),
    );
    let symlink = tokio::spawn(
        symlink_target(&generation_handle, PathTarget::at(&directory, "link")).unwrap(),
    );
    directory_gate.wait_started().await;
    attributes_gate.wait_started().await;
    symlink_gate.wait_started().await;
    control.push_release(Ok(()));
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));

    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::task::yield_now().await;
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));
    directory_gate.release();
    attributes_gate.release();
    symlink_gate.release();
    listing.await.unwrap().unwrap();
    attributes.await.unwrap().unwrap();
    symlink.await.unwrap().unwrap();
    drop(release(OpenNode::File(file)));
    drop(release(OpenNode::Directory(directory)));
    deletion.await.unwrap().unwrap();
}

#[test]
#[timeout("5s")]
async fn terminal_file_read_failure_invalidates_generation_handle() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 30).await;
    control.push_read(Err(sandbox_error(
        "read",
        std::io::ErrorKind::PermissionDenied,
    )));

    assert!(matches!(
        read_file(
            &generation_handle,
            &file,
            ReadRange {
                offset: 0,
                length: 1,
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "after-terminal-read").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        ),
        Err(AccessError::Revoked)
    ));

    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn terminal_attribute_query_failure_invalidates_generation_handle() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let target = PathTarget::at_root(&generation_handle, "attributes").unwrap();
    control.push_get_attributes(Err(sandbox_error(
        "get attributes",
        std::io::ErrorKind::InvalidData,
    )));

    assert!(matches!(
        attributes(&generation_handle, Target::Path(&target, Follow::No))
            .unwrap()
            .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "after-terminal-attributes").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        ),
        Err(AccessError::Revoked)
    ));

    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn guest_attribute_query_failure_remains_sandbox_and_keeps_generation_handle_valid() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let target = PathTarget::at_root(&generation_handle, "missing").unwrap();
    control.push_get_attributes(Err(sandbox_error(
        "get attributes",
        std::io::ErrorKind::NotFound,
    )));

    assert!(matches!(
        attributes(&generation_handle, Target::Path(&target, Follow::No))
            .unwrap()
            .await,
        Err(Error::Sandbox(_))
    ));
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "after-guest-query-failure").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("guest query failure invalidated filesystem generation handle");
    drop(admitted);

    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn limit_transition_revokes_admission_until_it_finishes() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let finite = limits(100, 10);
    control.push_install_limits(Ok(InstalledLimits {
        limits: finite,
        allocation: allocation(10, 1),
    }));
    let gate = control.block("install_limits");
    let transition = set_limits(filesystem, ResolvedStorageLimits::Finite(finite));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "during-transition").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        ),
        Err(AccessError::Transitioning)
    ));
    let transition = tokio::spawn(transition);
    gate.wait_started().await;
    gate.release();
    let filesystem = match transition.await.unwrap().unwrap() {
        LimitTransition::Resident(filesystem) => filesystem,
        LimitTransition::MustUnload(_) => panic!("transition unexpectedly required unload"),
    };
    let resumed = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "after-transition").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("limit transition did not resume admission");
    drop(resumed);
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropping_limit_observer_never_reopens_admission_and_deletes() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let finite = limits(100, 10);
    control.push_install_limits(Ok(InstalledLimits {
        limits: finite,
        allocation: allocation(10, 1),
    }));
    control.push_delete_and_verify(Ok(()));
    let gate = control.block("install_limits");

    let transition = set_limits(filesystem, ResolvedStorageLimits::Finite(finite));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "during-limit-change").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        ),
        Err(AccessError::Transitioning)
    ));
    drop(transition);
    gate.wait_started().await;
    gate.release();

    wait_for_call(&control, "delete_and_verify(").await;
    assert!(generation_handle.generation.upgrade().is_none());
}

#[test]
#[timeout("5s")]
async fn open_admitted_before_limit_change_returns_its_node_while_transitioning() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_open(Ok(SandboxOpened::scripted_file(24)));
    let open_gate = control.block("open");
    let opening = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "admitted-before-transition").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::ReadWrite,
                follow: Follow::Yes,
            },
        )
        .unwrap(),
    );
    open_gate.wait_started().await;

    let finite = limits(100, 10);
    control.push_install_limits(Ok(InstalledLimits {
        limits: finite,
        allocation: allocation(10, 1),
    }));
    let transition = set_limits(filesystem, ResolvedStorageLimits::Finite(finite));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "transition-probe").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        ),
        Err(AccessError::Transitioning)
    ));
    open_gate.release();

    let file = match opening.await.unwrap().unwrap().node {
        OpenNode::File(file) => file,
        OpenNode::Directory(_) => panic!("scripted open returned a directory"),
    };
    let filesystem = match transition.await.unwrap() {
        LimitTransition::Resident(filesystem) => filesystem,
        LimitTransition::MustUnload(_) => panic!("transition unexpectedly required unload"),
    };
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn limit_failure_seals_and_returns_ownership() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_install_limits(Err(sandbox_error(
        "install limits",
        std::io::ErrorKind::Other,
    )));
    let failure = set_limits(filesystem, ResolvedStorageLimits::Finite(limits(100, 10)))
        .await
        .unwrap_err();

    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "after-failure").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        ),
        Err(AccessError::Revoked)
    ));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn limit_transition_rejects_a_non_exact_sandbox_installation() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let requested = limits(100, 10);
    control.push_install_limits(Ok(InstalledLimits {
        limits: limits(99, 10),
        allocation: allocation(10, 1),
    }));

    let failure = set_limits(filesystem, ResolvedStorageLimits::Finite(requested))
        .await
        .unwrap_err();
    assert!(matches!(failure.source, Error::RuntimeInvalidated));
    control.push_delete_and_verify(Ok(()));
    delete(failure.filesystem).await.unwrap();
}

#[test]
async fn directory_insert_preserves_decisive_already_exists_error() {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_open(Ok(SandboxOpened::scripted_directory(23)));
    let directory = match open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "parent").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::ReadWrite,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap()
    .node
    {
        OpenNode::Directory(directory) => directory,
        OpenNode::File(_) => panic!("scripted open returned a file"),
    };
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_create_directory(Err(sandbox_error(
        "create directory",
        std::io::ErrorKind::AlreadyExists,
    )));

    assert!(matches!(
        edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "existing"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await,
        Err(Error::Sandbox(error))
            if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("create_directory("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn symlink_insert_preserves_decisive_already_exists_error() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 33).await;
    let desired = SandboxSymlinkTarget(PathBuf::from("target"));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_read_link(Ok(desired.clone()));
    control.push_create_symlink(Err(sandbox_error(
        "create symlink",
        std::io::ErrorKind::AlreadyExists,
    )));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "link"),
                object: NewObject::Symlink(SymlinkTarget(desired.0)),
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error))
            if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists)
    ));
    let calls = control.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("create_symlink("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn missing_remove_executes_sandbox_and_preserves_not_found() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 34).await;
    control.push_get_attributes(Err(missing("missing before remove")));
    control.push_unlink_file(Err(missing("unlink missing target")));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "missing"),
                expected: ObjectKind::File,
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error)) if error.io_kind() == Some(std::io::ErrorKind::NotFound)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("unlink_file("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn ambiguous_insert_from_absent_to_desired_state_is_accepted() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(40, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 35).await;

    control.push_get_attributes(Err(missing("directory before ambiguous insert")));
    control.push_create_directory(Err(sandbox_error(
        "ambiguous directory insert",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_observe_allocation(Ok(allocation(41, 2)));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "directory"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();

    let desired = SandboxSymlinkTarget(PathBuf::from("target"));
    control.push_get_attributes(Err(missing("symlink before ambiguous insert")));
    control.push_create_symlink(Err(sandbox_error(
        "ambiguous symlink insert",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_read_link(Ok(desired.clone()));
    control.push_observe_allocation(Ok(allocation(42, 3)));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "symlink"),
            object: NewObject::Symlink(SymlinkTarget(desired.0)),
        },
    )
    .unwrap()
    .await
    .unwrap();

    let calls = control.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("create_directory("))
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("create_symlink("))
            .count(),
        1
    );

    control.push_observe_allocation(Ok(allocation(42, 3)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn decisive_namespace_guest_errors_skip_observation_and_pressure_recovery() {
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Unlimited,
        allocation(50, 4),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 36).await;
    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("observe_allocation("))
        .count();

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_create_directory(Err(sandbox_error(
        "create existing directory",
        std::io::ErrorKind::AlreadyExists,
    )));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "existing-directory"),
                object: NewObject::Directory,
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error))
            if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists)
    ));

    let desired = SandboxSymlinkTarget(PathBuf::from("target"));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_read_link(Ok(desired.clone()));
    control.push_create_symlink(Err(sandbox_error(
        "create existing symlink",
        std::io::ErrorKind::AlreadyExists,
    )));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "existing-symlink"),
                object: NewObject::Symlink(SymlinkTarget(desired.0)),
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error))
            if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists)
    ));

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_hard_link(Err(sandbox_error(
        "hard link existing destination",
        std::io::ErrorKind::AlreadyExists,
    )));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at(&directory, "source"),
                destination: PathTarget::at(&directory, "existing-hard-link"),
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error))
            if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists)
    ));

    control.push_get_attributes(Err(missing("target before missing remove")));
    control.push_unlink_file(Err(missing("unlink missing target")));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "missing"),
                expected: ObjectKind::File,
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error)) if error.io_kind() == Some(std::io::ErrorKind::NotFound)
    ));

    assert_eq!(recovery.calls(), 0);
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        observations_before
    );

    control.push_observe_allocation(Ok(allocation(50, 4)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn initially_absent_remove_is_no_effect_not_desired_completion() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(60, 4)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 37).await;
    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("observe_allocation("))
        .count();
    control.push_get_attributes(Err(missing("target before remove")));
    control.push_unlink_file(Err(sandbox_error(
        "ambiguous unlink",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Err(missing("target after ambiguous unlink")));
    control.push_unlink_file(Err(missing("retry sees missing target")));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "missing"),
                expected: ObjectKind::File,
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error)) if error.io_kind() == Some(std::io::ErrorKind::NotFound)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("unlink_file("))
            .count(),
        2
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        observations_before
    );

    control.push_observe_allocation(Ok(allocation(60, 4)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn hard_link_preexisting_destination_is_no_effect_without_identity_inference() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 38).await;
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_hard_link(Err(sandbox_error(
        "ambiguous hard link",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_hard_link(Err(sandbox_error(
        "hard link existing destination",
        std::io::ErrorKind::AlreadyExists,
    )));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at(&directory, "source"),
                destination: PathTarget::at(&directory, "existing"),
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(error))
            if error.io_kind() == Some(std::io::ErrorKind::AlreadyExists)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("hard_link("))
            .count(),
        2
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn ambiguous_symlink_insert_requires_the_exact_target() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 39).await;
    control.push_get_attributes(Err(missing("symlink before ambiguous insert")));
    control.push_create_symlink(Err(sandbox_error(
        "ambiguous symlink insert",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_read_link(Ok(SandboxSymlinkTarget(PathBuf::from("other-target"))));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "symlink"),
                object: NewObject::Symlink(SymlinkTarget(PathBuf::from("target"))),
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("create_symlink("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_hard_link_effect_invalidates_without_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_get_attributes(Err(missing("hard-link destination before")));
    control.push_hard_link(Err(sandbox_error("hard link", std::io::ErrorKind::Other)));
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 2,
        size: 0,
        accessed: None,
        modified: None,
    }));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at_root(&generation_handle, "source").unwrap(),
                destination: PathTarget::at_root(&generation_handle, "destination").unwrap(),
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("hard_link("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[cfg(target_os = "linux")]
#[test]
async fn terminal_raw_namespace_error_invalidates_despite_no_effect_postcondition() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory(&generation_handle, &control, 40).await;
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_hard_link(Err(FilesystemStorageError::io(
        "terminal hard link",
        Path::new("<scripted>"),
        std::io::Error::from_raw_os_error(libc::EIO),
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at(&directory, "source"),
                destination: PathTarget::at(&directory, "existing"),
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("hard_link("))
            .count(),
        1
    );
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "after-terminal"),
                expected: ObjectKind::File,
            },
        ),
        Err(AccessError::Revoked)
    ));

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn rename_guest_failure_and_time_postcondition_keep_generation_handle_valid() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_get_attributes(Err(sandbox_error(
        "rename source",
        std::io::ErrorKind::NotFound,
    )));
    control.push_rename(Err(sandbox_error("rename", std::io::ErrorKind::NotFound)));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Move {
                source: PathTarget::at_root(&generation_handle, "missing").unwrap(),
                destination: PathTarget::at_root(&generation_handle, "destination").unwrap(),
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("rename("))
            .count(),
        1
    );

    let node = OpenNode::File(open_file(&generation_handle, &control, 34).await);
    let accessed = std::time::UNIX_EPOCH + Duration::from_secs(10);
    let modified = std::time::UNIX_EPOCH + Duration::from_secs(20);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: None,
        modified: None,
    }));
    control.push_set_times(Err(sandbox_error("set times", std::io::ErrorKind::Other)));
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: Some(accessed),
        modified: Some(modified),
    }));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::Times(TimeChanges {
            accessed: TimeChange::Set(accessed),
            modified: TimeChange::Set(modified),
        }),
    )
    .unwrap()
    .await
    .unwrap();
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "after-time-postcondition").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("proven namespace or time result invalidated filesystem generation handle");
    drop(admitted);

    control.push_release(Ok(()));
    release(node).await.unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_mutating_open_effect_invalidates_without_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_open(Err(sandbox_error("open", std::io::ErrorKind::Other)));
    control.push_get_attributes(Err(sandbox_error(
        "open postcondition",
        std::io::ErrorKind::NotFound,
    )));

    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "uncertain-create").unwrap(),
            OpenOptions::File {
                access: AccessMode::ReadWrite,
                disposition: FileDisposition::CreateIfMissing,
                follow: Follow::Yes,
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("open("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_attribute_effect_invalidates_without_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 25).await);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: None,
        modified: None,
    }));
    control.push_set_size(Err(sandbox_error("set size", std::io::ErrorKind::Other)));
    control.push_get_attributes(Err(sandbox_error(
        "size postcondition",
        std::io::ErrorKind::NotFound,
    )));

    assert!(matches!(
        set_attributes(
            &generation_handle,
            Target::Open(&node),
            AttributeChanges::File {
                size: 64,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("set_size("))
            .count(),
        1
    );

    control.push_release(Ok(()));
    release(node).await.unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_namespace_effect_invalidates_without_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: None,
        modified: None,
    }));
    control.push_get_attributes(Err(sandbox_error(
        "rename source after",
        std::io::ErrorKind::NotFound,
    )));
    control.push_get_attributes(Err(sandbox_error(
        "rename destination after",
        std::io::ErrorKind::NotFound,
    )));
    control.push_rename(Err(sandbox_error("rename", std::io::ErrorKind::Other)));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Move {
                source: PathTarget::at_root(&generation_handle, "source").unwrap(),
                destination: PathTarget::at_root(&generation_handle, "destination").unwrap(),
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("rename("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn hard_link_keeps_exact_resolved_source_and_destination_roles() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let source_parent = open_directory_at(
        &generation_handle,
        &control,
        801,
        AccessMode::ReadWrite,
        "source-parent",
    )
    .await;
    let destination_parent = open_directory_at(
        &generation_handle,
        &control,
        802,
        AccessMode::ReadWrite,
        "destination-parent",
    )
    .await;
    control.push_get_attributes(Err(missing("hard-link destination before")));
    control.push_hard_link(Ok(()));

    edit_namespace(
        &generation_handle,
        NamespaceEdit::Link {
            source: PathTarget::at(&source_parent, "source"),
            destination: PathTarget::at(&destination_parent, "destination"),
        },
    )
    .unwrap()
    .await
    .unwrap();

    assert_eq!(
        control.sandbox_path_calls(),
        vec![ScriptedSandboxPathCall::HardLink {
            source: ScriptedSandboxPath {
                base: ScriptedSandboxPathBase::Directory(801),
                path: PathBuf::from("source"),
            },
            destination: ScriptedSandboxPath {
                base: ScriptedSandboxPathBase::Directory(802),
                path: PathBuf::from("destination"),
            },
        }]
    );
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(source_parent)).await.unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(destination_parent))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn rename_keeps_exact_resolved_source_and_destination_roles() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let source_parent = open_directory_at(
        &generation_handle,
        &control,
        811,
        AccessMode::ReadWrite,
        "source-parent",
    )
    .await;
    let destination_parent = open_directory_at(
        &generation_handle,
        &control,
        812,
        AccessMode::ReadWrite,
        "destination-parent",
    )
    .await;
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_rename(Ok(()));

    edit_namespace(
        &generation_handle,
        NamespaceEdit::Move {
            source: PathTarget::at(&source_parent, "source"),
            destination: PathTarget::at(&destination_parent, "destination"),
        },
    )
    .unwrap()
    .await
    .unwrap();

    assert_eq!(
        control.sandbox_path_calls(),
        vec![ScriptedSandboxPathCall::Rename {
            source: ScriptedSandboxPath {
                base: ScriptedSandboxPathBase::Directory(811),
                path: PathBuf::from("source"),
            },
            destination: ScriptedSandboxPath {
                base: ScriptedSandboxPathBase::Directory(812),
                path: PathBuf::from("destination"),
            },
        }]
    );
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(source_parent)).await.unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(destination_parent))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn every_namespace_operation_executes_without_billing_observation() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(10, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        80,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    let observations_before = call_count(&control, "observe_allocation(");

    control.push_get_attributes(Err(missing("directory before insert")));
    control.push_create_directory(Ok(()));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "directory"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_get_attributes(Err(missing("symlink before insert")));
    control.push_create_symlink(Ok(()));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "symlink"),
            object: NewObject::Symlink(SymlinkTarget(PathBuf::from("target"))),
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_get_attributes(Err(missing("hard-link destination before")));
    control.push_hard_link(Ok(()));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Link {
            source: PathTarget::at(&directory, "source"),
            destination: PathTarget::at(&directory, "hard-link"),
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_rename(Ok(()));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Move {
            source: PathTarget::at(&directory, "before"),
            destination: PathTarget::at(&directory, "after"),
        },
    )
    .unwrap()
    .await
    .unwrap();

    for (path, expected, sandbox_kind) in [
        (
            "old-directory",
            ObjectKind::Directory,
            SandboxObjectKind::Directory,
        ),
        ("old-file", ObjectKind::File, SandboxObjectKind::File),
        (
            "old-symlink",
            ObjectKind::Symlink,
            SandboxObjectKind::Symlink,
        ),
    ] {
        control.push_get_attributes(Ok(sandbox_attributes(sandbox_kind)));
        match expected {
            ObjectKind::Directory => control.push_remove_directory(Ok(())),
            ObjectKind::File | ObjectKind::Symlink => control.push_unlink_file(Ok(())),
        }
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, path),
                expected,
            },
        )
        .unwrap()
        .await
        .unwrap();
    }

    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn namespace_authorization_and_expected_kind_reject_before_sandbox_mutation() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let read_only = open_directory_at(
        &generation_handle,
        &control,
        81,
        AccessMode::Read,
        "read-only",
    )
    .await;
    let writable = open_directory_at(
        &generation_handle,
        &control,
        82,
        AccessMode::ReadWrite,
        "writable",
    )
    .await;

    assert_eq!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&read_only, "child"),
                object: NewObject::Directory,
            },
        )
        .unwrap_err(),
        AccessError::NotPermitted
    );
    assert_eq!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at(&read_only, "source"),
                destination: PathTarget::at(&writable, "destination"),
            },
        )
        .unwrap_err(),
        AccessError::NotPermitted
    );
    assert!(!has_call(&control, "create_directory("));
    assert!(!has_call(&control, "hard_link("));

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&writable, "wrong-kind"),
                expected: ObjectKind::File,
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert!(!has_call(&control, "unlink_file("));

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    control.push_release(Ok(()));
    release(OpenNode::Directory(read_only)).await.unwrap();
    release(OpenNode::Directory(writable)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn semantic_initial_file_policy_covers_root_descriptor_and_alias_targets() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let generation = generation_handle.generation.upgrade().unwrap();
    generation.initial_files.lock().unwrap().insert(
        PathBuf::from("logical/read-only"),
        InitialAgentFile {
            content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
            path: AgentFilePath::from_abs_str("/logical/read-only").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            size: 0,
        },
    );
    let parent = open_directory_at(
        &generation_handle,
        &control,
        120,
        AccessMode::ReadWrite,
        "backend/physical/parent",
    )
    .await;

    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    let root_write = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "backend/physical/read-only").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Write,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await;
    assert!(matches!(
        root_write,
        Err(Error::Access(AccessError::NotPermitted))
    ));

    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    let parent_write = open(
        &generation_handle,
        PathTarget::at(&parent, "read-only"),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Write,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await;
    assert!(matches!(
        parent_write,
        Err(Error::Access(AccessError::NotPermitted))
    ));

    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    let parent_unlink = edit_namespace(
        &generation_handle,
        NamespaceEdit::Remove {
            target: PathTarget::at(&parent, "read-only"),
            expected: ObjectKind::File,
        },
    )
    .unwrap()
    .await;
    assert!(matches!(
        parent_unlink,
        Err(Error::Access(AccessError::NotPermitted))
    ));

    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    control.push_policy_resolution(700, "renamed", None, None);
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    let parent_rename = edit_namespace(
        &generation_handle,
        NamespaceEdit::Move {
            source: PathTarget::at(&parent, "read-only"),
            destination: PathTarget::at(&parent, "renamed"),
        },
    )
    .unwrap()
    .await;
    assert!(matches!(
        parent_rename,
        Err(Error::Access(AccessError::NotPermitted))
    ));

    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    control.push_policy_resolution(700, "hard-link", None, None);
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    let parent_link = edit_namespace(
        &generation_handle,
        NamespaceEdit::Link {
            source: PathTarget::at(&parent, "read-only"),
            destination: PathTarget::at(&parent, "hard-link"),
        },
    )
    .unwrap()
    .await;
    assert!(matches!(
        parent_link,
        Err(Error::Access(AccessError::NotPermitted))
    ));

    control.push_policy_resolution(700, "alias", Some(901), Some(900));
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    let alias_write = open(
        &generation_handle,
        PathTarget::at(&parent, "alias"),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Write,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await;
    assert!(matches!(
        alias_write,
        Err(Error::Access(AccessError::NotPermitted))
    ));

    control.push_policy_resolution(700, "alias", Some(901), Some(900));
    control.push_policy_resolution(700, "read-only", Some(900), Some(900));
    control.push_namespace_resolution(700, "alias", None);
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_unlink_file(Ok(()));
    control.push_observe_allocation(Ok(allocation(0, 0)));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Remove {
            target: PathTarget::at(&parent, "alias"),
            expected: ObjectKind::File,
        },
    )
    .unwrap()
    .await
    .unwrap();

    assert_eq!(call_count(&control, "unlink_file("), 1);
    assert!(!has_call(&control, "rename("));
    assert!(!has_call(&control, "hard_link("));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(parent)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn wrong_generation_namespace_paths_reject_without_sandbox_work_or_invalidation() {
    let (first, first_control, first_window) = metered_resident().await;
    let (second, second_control, second_window) = metered_resident().await;
    let first_generation_handle = resident_generation_handle(&first);
    let second_generation_handle = resident_generation_handle(&second);
    let foreign = PathTarget::at_root(&second_generation_handle, "foreign").unwrap();

    assert_eq!(
        edit_namespace(
            &first_generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at_root(&first_generation_handle, "source").unwrap(),
                destination: foreign,
            },
        )
        .unwrap_err(),
        AccessError::WrongGeneration
    );
    assert!(!has_call(&first_control, "hard_link("));
    assert!(!has_call(&second_control, "hard_link("));
    let admitted = open(
        &first_generation_handle,
        PathTarget::at_root(&first_generation_handle, "still-valid").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("cross-generation link invalidated the source generation");
    drop(admitted);

    close_window(first_window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    close_window(second_window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    first_control.push_delete_and_verify(Ok(()));
    second_control.push_delete_and_verify(Ok(()));
    delete(seal(first)).await.unwrap();
    delete(seal(second)).await.unwrap();
}

#[test]
async fn namespace_resolution_errors_precede_mutation_and_preserve_terminal_classification() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_namespace_resolution_error(missing("resolve missing parent"));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at_root(&generation_handle, "missing/child").unwrap(),
                object: NewObject::Directory,
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert!(!has_call(&control, "create_directory("));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();

    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_namespace_resolution_error(sandbox_error(
        "resolve corrupt parent",
        std::io::ErrorKind::InvalidData,
    ));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at_root(&generation_handle, "target").unwrap(),
                expected: ObjectKind::File,
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert!(!has_call(&control, "unlink_file("));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn namespace_coordination_uses_semantic_name_sets_and_keeps_unrelated_names_concurrent() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        83,
        AccessMode::ReadWrite,
        "root",
    )
    .await;

    control.push_get_attributes(Err(missing("exact insert before")));
    control.push_create_directory(Ok(()));
    let exact_gate = control.block("create_directory");
    let exact_first = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "same"),
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    exact_gate.wait_started().await;
    let attributes_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("get_path_attributes("))
        .count();
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_remove_directory(Ok(()));
    let exact_second = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "./same"),
                expected: ObjectKind::Directory,
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("get_path_attributes("))
            .count(),
        attributes_before
    );
    exact_gate.release();
    exact_first.await.unwrap().unwrap();
    exact_second.await.unwrap().unwrap();

    control.push_get_attributes(Err(missing("two-path link destination before")));
    control.push_hard_link(Ok(()));
    let two_path_gate = control.block("hard_link");
    let two_path_first = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at(&directory, "left"),
                destination: PathTarget::at(&directory, "shared"),
            },
        )
        .unwrap(),
    );
    two_path_gate.wait_started().await;
    let before_move_observation = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("get_path_attributes("))
        .count();
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_rename(Ok(()));
    let two_path_second = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Move {
                source: PathTarget::at(&directory, "right"),
                destination: PathTarget::at(&directory, "folder/../shared"),
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("get_path_attributes("))
            .count(),
        before_move_observation
    );
    two_path_gate.release();
    two_path_first.await.unwrap().unwrap();
    two_path_second.await.unwrap().unwrap();

    control.push_get_attributes(Err(missing("unrelated-a before")));
    control.push_get_attributes(Err(missing("unrelated-b before")));
    control.push_create_directory(Ok(()));
    control.push_create_directory(Ok(()));
    let unrelated_gate = control.block("create_directory");
    let unrelated_first = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "unrelated-a"),
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    unrelated_gate.wait_started().await;
    let unrelated_second = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "unrelated-b"),
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(1), unrelated_second)
        .await
        .expect("unrelated namespace edit was serialized")
        .unwrap()
        .unwrap();
    assert!(!unrelated_first.is_finished());
    unrelated_gate.release();
    unrelated_first.await.unwrap().unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn ambiguous_remove_retry_excludes_a_conflicting_file_recreation() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        100,
        AccessMode::ReadWrite,
        "root",
    )
    .await;

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Err(sandbox_error(
        "ambiguous unlink",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Ok(()));
    let first_unlink = control.block("unlink_file");
    let removing = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "same"),
                expected: ObjectKind::File,
            },
        )
        .unwrap(),
    );
    first_unlink.wait_started().await;

    let postcondition = control.block("get_path_attributes");
    let retry = control.block("unlink_file");
    first_unlink.release();
    postcondition.wait_started().await;
    let opens_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("open("))
        .count();
    control.push_open(Ok(SandboxOpened::scripted_file(101)));
    let recreating = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at(&directory, "same"),
            OpenOptions::File {
                access: AccessMode::ReadWrite,
                disposition: FileDisposition::CreateOrTruncate,
                follow: Follow::Yes,
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("open("))
            .count(),
        opens_before
    );

    postcondition.release();
    retry.wait_started().await;
    assert!(!recreating.is_finished());
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("open("))
            .count(),
        opens_before
    );
    retry.release();
    removing.await.unwrap().unwrap();
    let recreated = recreating.await.unwrap().unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(recreated.node).await.unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn rename_evidence_excludes_a_conflicting_file_recreation() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        102,
        AccessMode::ReadWrite,
        "root",
    )
    .await;

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_rename(Err(sandbox_error(
        "ambiguous rename",
        std::io::ErrorKind::Other,
    )));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));
    control.push_get_attributes(Err(missing("rename destination remains absent")));
    control.push_rename(Ok(()));
    let first_rename = control.block("rename");
    let moving = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Move {
                source: PathTarget::at(&directory, "source"),
                destination: PathTarget::at(&directory, "destination"),
            },
        )
        .unwrap(),
    );
    first_rename.wait_started().await;

    let postcondition = control.block("get_path_attributes");
    let retry = control.block("rename");
    first_rename.release();
    postcondition.wait_started().await;
    let opens_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("open("))
        .count();
    control.push_open(Ok(SandboxOpened::scripted_file(103)));
    let recreating = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at(&directory, "destination"),
            OpenOptions::File {
                access: AccessMode::ReadWrite,
                disposition: FileDisposition::CreateIfMissing,
                follow: Follow::Yes,
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("open("))
            .count(),
        opens_before
    );

    postcondition.release();
    retry.wait_started().await;
    assert!(!recreating.is_finished());
    retry.release();
    moving.await.unwrap().unwrap();
    let recreated = recreating.await.unwrap().unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(recreated.node).await.unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn file_creation_open_blocks_only_conflicting_namespace_edits() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        104,
        AccessMode::ReadWrite,
        "root",
    )
    .await;

    control.push_open(Ok(SandboxOpened::scripted_file(105)));
    let open_gate = control.block("open");
    let opening = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at(&directory, "same"),
            OpenOptions::File {
                access: AccessMode::ReadWrite,
                disposition: FileDisposition::CreateExclusive,
                follow: Follow::No,
            },
        )
        .unwrap(),
    );
    open_gate.wait_started().await;

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Ok(()));
    tokio::time::timeout(
        Duration::from_secs(1),
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "other"),
                expected: ObjectKind::File,
            },
        )
        .unwrap(),
    )
    .await
    .expect("unrelated namespace edit was serialized")
    .unwrap();
    assert!(!opening.is_finished());

    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("get_path_attributes("))
        .count();
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Ok(()));
    let conflicting = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "same"),
                expected: ObjectKind::File,
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("get_path_attributes("))
            .count(),
        observations_before
    );

    open_gate.release();
    let opened = opening.await.unwrap().unwrap();
    conflicting.await.unwrap().unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(opened.node).await.unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn existing_file_open_does_not_block_a_namespace_edit() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        113,
        AccessMode::ReadWrite,
        "root",
    )
    .await;

    control.push_open(Ok(SandboxOpened::scripted_file(114)));
    let open_gate = control.block("open");
    let opening = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at(&directory, "same"),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        )
        .unwrap(),
    );
    open_gate.wait_started().await;

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Ok(()));
    tokio::time::timeout(
        Duration::from_secs(1),
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "same"),
                expected: ObjectKind::File,
            },
        )
        .unwrap(),
    )
    .await
    .expect("existing file open serialized a namespace edit")
    .unwrap();
    assert!(!opening.is_finished());

    open_gate.release();
    let opened = opening.await.unwrap().unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(opened.node).await.unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
fn every_file_disposition_is_a_namespace_edit_while_existing_files_are_not() {
    for disposition in [
        FileDisposition::CreateIfMissing,
        FileDisposition::CreateExclusive,
        FileDisposition::TruncateExisting,
        FileDisposition::CreateOrTruncate,
    ] {
        assert_eq!(
            open_namespace_coordination(OpenOptions::File {
                access: AccessMode::ReadWrite,
                disposition,
                follow: Follow::Yes,
            }),
            Some(NamespaceCoordinationKind::Edit)
        );
    }
    assert_eq!(
        open_namespace_coordination(OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        }),
        None
    );
}

#[test]
#[timeout("10s")]
async fn direct_and_alias_descriptors_use_the_same_directory_identity() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let direct =
        open_directory_with_identity(&generation_handle, &control, 90, 700, "direct").await;
    let alias = open_directory_with_identity(&generation_handle, &control, 91, 700, "alias").await;

    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&direct, "same"),
        PathTarget::at(&alias, "same"),
        true,
    )
    .await;

    let reused =
        open_directory_with_identity(&generation_handle, &control, 92, 701, "direct").await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&direct, "independent"),
        PathTarget::at(&reused, "independent"),
        false,
    )
    .await;

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    for directory in [direct, alias, reused] {
        control.push_release(Ok(()));
        release(OpenNode::Directory(directory)).await.unwrap();
    }
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn target_move_and_remove_conflict_with_descendant_edits_through_aliases() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let direct =
        open_directory_with_identity(&generation_handle, &control, 93, 710, "direct").await;
    let alias = open_directory_with_identity(&generation_handle, &control, 94, 710, "alias").await;

    control.push_namespace_resolution(1, "source", None);
    control.push_namespace_resolution(1, "destination", None);
    control.push_namespace_resolution(1, "source", Some(710));
    control.push_namespace_resolution(1, "destination", None);
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_rename(Ok(()));
    let rename_gate = control.block("rename");
    let moving = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Move {
                source: PathTarget::at_root(&generation_handle, "source").unwrap(),
                destination: PathTarget::at_root(&generation_handle, "destination").unwrap(),
            },
        )
        .unwrap(),
    );
    rename_gate.wait_started().await;

    control.push_get_attributes(Err(missing("aliased child remains absent")));
    control.push_create_directory(Ok(()));
    let descendant = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&alias, "child"),
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!descendant.is_finished());
    rename_gate.release();
    moving.await.unwrap().unwrap();
    descendant.await.unwrap().unwrap();

    control.push_namespace_resolution(1, "target", None);
    control.push_namespace_resolution(1, "target", Some(710));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_remove_directory(Ok(()));
    let remove_gate = control.block("remove_directory");
    let removing = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at_root(&generation_handle, "target").unwrap(),
                expected: ObjectKind::Directory,
            },
        )
        .unwrap(),
    );
    remove_gate.wait_started().await;

    control.push_get_attributes(Err(missing("direct child remains absent")));
    control.push_create_directory(Ok(()));
    let descendant = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&direct, "other-child"),
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!descendant.is_finished());
    remove_gate.release();
    removing.await.unwrap().unwrap();
    descendant.await.unwrap().unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    for directory in [direct, alias] {
        control.push_release(Ok(()));
        release(OpenNode::Directory(directory)).await.unwrap();
    }
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn directory_key_extension_prevents_write_skew_without_deadlock() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let generation = generation_handle.generation.upgrade().unwrap();
    control.push_namespace_resolution(1, "entry", Some(900));
    control.push_namespace_resolution(900, "child", None);
    let entry = resolve_namespace_target(&generation, SandboxPath::at_root("entry"))
        .await
        .unwrap();
    let child = resolve_namespace_target(&generation, SandboxPath::at_root("child"))
        .await
        .unwrap();
    let mut entry_lease = generation
        .namespace
        .coordinate(
            NamespaceCoordinationKind::Edit,
            vec![entry.coordination_key()],
        )
        .await;
    let mut child_lease = generation
        .namespace
        .coordinate(
            NamespaceCoordinationKind::Edit,
            vec![child.coordination_key()],
        )
        .await;

    assert!(entry_lease.extend(vec![entry.final_directory_key().unwrap()]));
    assert!(!child_lease.extend(Vec::new()));
    drop(child_lease);
    drop(entry_lease);

    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn sandbox_equivalent_names_share_one_coordination_key() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        95,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    control.push_namespace_resolution(800, "sandbox-equivalent", None);
    control.push_namespace_resolution(800, "sandbox-equivalent", None);
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&directory, "CaseName"),
        PathTarget::at(&directory, "casename"),
        true,
    )
    .await;

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn conservative_sandbox_names_serialize_only_within_their_parent() {
    let coordinator = Arc::new(NamespaceCoordinator::new());
    let held = coordinator
        .coordinate(
            NamespaceCoordinationKind::Edit,
            vec![SandboxNamespaceCoordinationKey::scripted_conservative(
                1, "first",
            )],
        )
        .await;

    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            coordinator.coordinate(
                NamespaceCoordinationKind::Edit,
                vec![SandboxNamespaceCoordinationKey::scripted_conservative(
                    1, "second",
                )],
            ),
        )
        .await
        .is_err()
    );
    let other_parent = tokio::time::timeout(
        Duration::from_millis(20),
        coordinator.coordinate(
            NamespaceCoordinationKind::Edit,
            vec![SandboxNamespaceCoordinationKey::scripted_conservative(
                2, "second",
            )],
        ),
    )
    .await
    .expect("different parent identities must remain independent");

    drop(other_parent);
    drop(held);
    tokio::time::timeout(
        Duration::from_millis(20),
        coordinator.coordinate(
            NamespaceCoordinationKind::Edit,
            vec![SandboxNamespaceCoordinationKey::scripted_conservative(
                1, "second",
            )],
        ),
    )
    .await
    .expect("sibling must proceed after the conflicting lease is dropped");
}

#[test]
#[timeout("10s")]
async fn symlink_alias_descriptors_preserve_object_identity_across_entry_move() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let moved = open_directory_at(
        &generation_handle,
        &control,
        106,
        AccessMode::ReadWrite,
        "source-alias",
    )
    .await;
    let moved_descendant = open_directory_target(
        &generation_handle,
        &control,
        107,
        AccessMode::ReadWrite,
        PathTarget::at(&moved, "nested"),
    )
    .await;
    let displaced = open_directory_at(
        &generation_handle,
        &control,
        108,
        AccessMode::ReadWrite,
        "destination-alias",
    )
    .await;
    let displaced_descendant = open_directory_target(
        &generation_handle,
        &control,
        109,
        AccessMode::ReadWrite,
        PathTarget::at(&displaced, "nested"),
    )
    .await;

    move_namespace_entry(
        &generation_handle,
        &control,
        PathTarget::at_root(&generation_handle, "source-alias").unwrap(),
        PathTarget::at_root(&generation_handle, "destination-alias").unwrap(),
        SandboxObjectKind::Symlink,
    )
    .await;

    let current = open_directory_at(
        &generation_handle,
        &control,
        106,
        AccessMode::ReadWrite,
        "destination-alias",
    )
    .await;
    let current_descendant = open_directory_target(
        &generation_handle,
        &control,
        107,
        AccessMode::ReadWrite,
        PathTarget::at(&current, "nested"),
    )
    .await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&moved, "same"),
        PathTarget::at(&current, "same"),
        true,
    )
    .await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&moved_descendant, "same"),
        PathTarget::at(&current_descendant, "same"),
        true,
    )
    .await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&displaced, "independent"),
        PathTarget::at(&current, "independent"),
        false,
    )
    .await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&displaced_descendant, "independent"),
        PathTarget::at(&current_descendant, "independent"),
        false,
    )
    .await;

    let reused_source = open_directory_at(
        &generation_handle,
        &control,
        112,
        AccessMode::ReadWrite,
        "source-alias",
    )
    .await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&moved, "reused"),
        PathTarget::at(&reused_source, "reused"),
        false,
    )
    .await;

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    for directory in [
        moved,
        moved_descendant,
        displaced,
        displaced_descendant,
        current,
        current_descendant,
        reused_source,
    ] {
        control.push_release(Ok(()));
        release(OpenNode::Directory(directory)).await.unwrap();
    }
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("10s")]
async fn directory_open_registers_before_a_conflicting_move_can_publish() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_open(Ok(SandboxOpened::scripted_directory(98)));
    let open_gate = control.block("open");
    let opening = tokio::spawn(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "old").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::Directory,
                access: AccessMode::ReadWrite,
                follow: Follow::Yes,
            },
        )
        .unwrap(),
    );
    open_gate.wait_started().await;

    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("get_path_attributes("))
        .count();
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Directory)));
    control.push_rename(Ok(()));
    let moving = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Move {
                source: PathTarget::at_root(&generation_handle, "old").unwrap(),
                destination: PathTarget::at_root(&generation_handle, "new").unwrap(),
            },
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("get_path_attributes("))
            .count(),
        observations_before
    );

    open_gate.release();
    let opened = opening.await.unwrap().unwrap();
    moving.await.unwrap().unwrap();
    let raced = match opened.node {
        OpenNode::Directory(directory) => directory,
        OpenNode::File(_) => panic!("scripted open returned a file"),
    };
    let current = open_directory_at(
        &generation_handle,
        &control,
        98,
        AccessMode::ReadWrite,
        "new",
    )
    .await;
    assert_insert_coordination(
        &generation_handle,
        &control,
        PathTarget::at(&raced, "same"),
        PathTarget::at(&current, "same"),
        true,
    )
    .await;

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    for directory in [raced, current] {
        control.push_release(Ok(()));
        release(OpenNode::Directory(directory)).await.unwrap();
    }
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn namespace_postconditions_retry_only_no_effect_and_accept_desired_state() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        84,
        AccessMode::ReadWrite,
        "root",
    )
    .await;

    control.push_create_directory(Err(sandbox_error(
        "create directory",
        std::io::ErrorKind::WouldBlock,
    )));
    control.push_get_attributes(Err(missing("directory remains absent")));
    control.push_create_directory(Ok(()));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "retry-directory"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_hard_link(Err(sandbox_error("hard link", std::io::ErrorKind::Other)));
    control.push_get_attributes(Err(missing("hard-link destination remains absent")));
    control.push_get_attributes(Err(missing("hard-link destination remains absent")));
    control.push_hard_link(Ok(()));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Link {
            source: PathTarget::at(&directory, "source"),
            destination: PathTarget::at(&directory, "retry-link"),
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_rename(Err(sandbox_error("rename", std::io::ErrorKind::Other)));
    control.push_get_attributes(Err(missing("rename source absent")));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Move {
            source: PathTarget::at(&directory, "move-source"),
            destination: PathTarget::at(&directory, "move-destination"),
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Err(sandbox_error("unlink", std::io::ErrorKind::Other)));
    control.push_get_attributes(Err(missing("removed path absent")));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Remove {
            target: PathTarget::at(&directory, "removed"),
            expected: ObjectKind::File,
        },
    )
    .unwrap()
    .await
    .unwrap();

    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("create_directory("))
            .count(),
        2
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("hard_link("))
            .count(),
        2
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("rename("))
            .count(),
        1
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("unlink_file("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn remove_unknown_kind_change_invalidates_without_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        85,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::File)));
    control.push_unlink_file(Err(sandbox_error("unlink", std::io::ErrorKind::Other)));
    control.push_get_attributes(Ok(sandbox_attributes(SandboxObjectKind::Symlink)));

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Remove {
                target: PathTarget::at(&directory, "changed"),
                expected: ObjectKind::File,
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("unlink_file("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn namespace_quota_precedes_pressure_and_proven_no_effect_can_recover_capacity() {
    let quota_recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(100, 1),
        Some(quota_recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        86,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    control.push_create_directory(Err(sandbox_error(
        "create directory",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_get_attributes(Err(missing("directory remains absent")));
    control.push_observe_allocation(Ok(allocation(100, 1)));
    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "quota"),
                object: NewObject::Directory,
            },
        )
        .unwrap()
        .await,
        Err(Error::AgentQuota(_))
    ));
    assert_eq!(quota_recovery.calls(), 0);
    control.push_observe_allocation(Ok(allocation(100, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();

    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        87,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    control.push_create_directory(Err(sandbox_error(
        "create directory",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_get_attributes(Err(missing("directory remains absent")));
    control.push_observe_allocation(Ok(allocation(80, 1)));
    control.push_create_directory(Ok(()));
    control.push_observe_allocation(Ok(allocation(81, 2)));
    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "pressure"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(recovery.calls(), 1);
    control.push_observe_allocation(Ok(allocation(81, 2)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn sandbox_cross_device_link_is_proven_no_effect_without_recovery_or_observation() {
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    control.push_hard_link(Err(sandbox_error(
        "hard link",
        std::io::ErrorKind::CrossesDevices,
    )));
    control.push_get_attributes(Err(missing("destination remains absent")));
    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("observe_allocation("))
        .count();

    assert!(matches!(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Link {
                source: PathTarget::at_root(&generation_handle, "source").unwrap(),
                destination: PathTarget::at_root(&generation_handle, "destination").unwrap(),
            },
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert_eq!(recovery.calls(), 0);
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        observations_before
    );

    control.push_observe_allocation(Ok(allocation(80, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn successful_namespace_edit_does_not_consume_a_queued_observation_failure() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(90, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        88,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    control.push_get_attributes(Err(missing("created path before")));
    control.push_create_directory(Ok(()));
    let observations_before = call_count(&control, "observe_allocation(");
    control.push_observe_allocation(Err(sandbox_error(
        "observe namespace allocation",
        std::io::ErrorKind::Other,
    )));

    edit_namespace(
        &generation_handle,
        NamespaceEdit::Insert {
            destination: PathTarget::at(&directory, "created"),
            object: NewObject::Directory,
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropped_namespace_observer_keeps_sandbox_work_without_billing_close_coupling() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(91, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let directory = open_directory_at(
        &generation_handle,
        &control,
        89,
        AccessMode::ReadWrite,
        "root",
    )
    .await;
    control.push_get_attributes(Err(missing("created path before")));
    control.push_create_directory(Ok(()));
    let observations_before = call_count(&control, "observe_allocation(");
    let gate = control.block("create_directory");
    let observer = tokio::spawn(
        edit_namespace(
            &generation_handle,
            NamespaceEdit::Insert {
                destination: PathTarget::at(&directory, "created"),
                object: NewObject::Directory,
            },
        )
        .unwrap(),
    );
    gate.wait_started().await;
    observer.abort();

    let close = tokio::spawn(close_window(
        window,
        Instant::now() + Duration::from_secs(1),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(close.is_finished());
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close.await.unwrap().unwrap();
    gate.release();
    gate.wait_completed().await;
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );
    control.push_release(Ok(()));
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_synchronization_effect_invalidates_without_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 26).await);
    control.push_synchronize(Err(sandbox_error("synchronize", std::io::ErrorKind::Other)));

    assert!(matches!(
        synchronize(&generation_handle, &node, Synchronization::DataAndMetadata)
            .unwrap()
            .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("synchronize("))
            .count(),
        1
    );

    control.push_release(Ok(()));
    release(node).await.unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_release_effect_invalidates_and_still_drains_deletion() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 27).await);
    control.push_release(Err(sandbox_error("release", std::io::ErrorKind::Other)));

    assert!(matches!(
        release(node).await,
        Err(Error::RuntimeInvalidated)
    ));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "after-release-failure").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        ),
        Err(AccessError::Revoked)
    ));

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn idle_in_limit_update_retains_resident_without_resource_window() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let finite = limits(100, 10);
    control.push_install_limits(Ok(InstalledLimits {
        limits: finite,
        allocation: allocation(10, 1),
    }));
    let filesystem = match set_limits(filesystem, ResolvedStorageLimits::Finite(finite))
        .await
        .unwrap()
    {
        LimitTransition::Resident(filesystem) => filesystem,
        LimitTransition::MustUnload(_) => panic!("limits unexpectedly required unload"),
    };
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "after-idle-limit-update").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("in-limit idle update replaced or revoked the resident generation");
    drop(admitted);
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn idle_downgrade_over_usage_requires_unload_without_resource_window() {
    let (filesystem, control, _) = resident(Ok(allocation(10, 1))).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let lower = limits(5, 1);
    control.push_install_limits(Ok(InstalledLimits {
        limits: lower,
        allocation: allocation(10, 1),
    }));
    let filesystem = match set_limits(filesystem, ResolvedStorageLimits::Finite(lower))
        .await
        .unwrap()
    {
        LimitTransition::MustUnload(filesystem) => filesystem,
        LimitTransition::Resident(_) => panic!("over-limit allocation remained resident"),
    };
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "after-over-limit-idle-update").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        ),
        Err(AccessError::Revoked)
    ));
    control.push_delete_and_verify(Ok(()));
    delete(filesystem).await.unwrap();
}

#[test]
async fn finite_limits_fail_on_unmanaged_production_storage_and_cleanup() {
    let parent = tempfile::tempdir().unwrap();
    let profile = FilesystemStorageConfig {
        deterministic_root_dir: Some(parent.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let id = agent_id();
    let root = parent
        .path()
        .join(id.environment_id.to_string())
        .join(id.agent_id.component_id.to_string())
        .join(id.agent_id.agent_name_encoded());

    let provisioning = sandbox_provisioning(&profile).unwrap();
    assert!(
        create_fresh(
            provisioning,
            id,
            ResolvedStorageLimits::Finite(limits(1024, 16))
        )
        .await
        .is_err()
    );
    assert!(!root.exists());
}

#[test]
async fn shared_provisioning_creates_distinct_typed_filesystems_with_independent_deletion() {
    let parent = tempfile::tempdir().unwrap();
    let profile = FilesystemStorageConfig {
        deterministic_root_dir: Some(parent.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let provisioning = sandbox_provisioning(&profile).unwrap();
    let first_id = agent_id();
    let second_id = agent_id();
    let first_root = parent
        .path()
        .join(first_id.environment_id.to_string())
        .join(first_id.agent_id.component_id.to_string())
        .join(first_id.agent_id.agent_name_encoded());
    let second_root = parent
        .path()
        .join(second_id.environment_id.to_string())
        .join(second_id.agent_id.component_id.to_string())
        .join(second_id.agent_id.agent_name_encoded());

    let first = create_fresh(
        provisioning.clone(),
        first_id,
        ResolvedStorageLimits::Unlimited,
    )
    .await
    .unwrap();
    let second = create_fresh(provisioning, second_id, ResolvedStorageLimits::Unlimited)
        .await
        .unwrap();

    assert!(first_root.is_dir());
    assert!(second_root.is_dir());
    delete_created(first).await.unwrap();
    assert!(!first_root.exists());
    assert!(second_root.is_dir());
    delete_created(second).await.unwrap();
    assert!(!second_root.exists());
}

#[test]
async fn unmanaged_reconstruction_materializes_initial_files_with_declared_permissions() {
    let parent = tempfile::tempdir().unwrap();
    let profile = FilesystemStorageConfig {
        deterministic_root_dir: Some(parent.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let id = agent_id();
    let root = parent
        .path()
        .join(id.environment_id.to_string())
        .join(id.agent_id.component_id.to_string())
        .join(id.agent_id.agent_name_encoded());
    let service = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let read_only = b"immutable initial file".to_vec();
    let read_write = b"mutable initial file".to_vec();
    let read_only_hash = service
        .put_if_not_exists(
            id.environment_id,
            read_only
                .clone()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await
        .unwrap();
    let read_write_hash = service
        .put_if_not_exists(
            id.environment_id,
            read_write
                .clone()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await
        .unwrap();
    let loader = Arc::new(FileLoader::new(service, None).unwrap());
    let files = vec![
        InitialAgentFile {
            content_hash: read_only_hash,
            path: AgentFilePath::from_abs_str("/read-only").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            size: read_only.len() as u64,
        },
        InitialAgentFile {
            content_hash: read_write_hash,
            path: AgentFilePath::from_abs_str("/read-write").unwrap(),
            permissions: AgentFilePermissions::ReadWrite,
            size: read_write.len() as u64,
        },
    ];
    let prepared = prepare_initial_files(&loader, id.environment_id, &files)
        .await
        .unwrap();
    let provisioning = sandbox_provisioning(&profile).unwrap();
    let created = create_fresh(provisioning, id.clone(), ResolvedStorageLimits::Unlimited)
        .await
        .unwrap();
    let (account, entry) = account();
    let reconstructing = bind_configured_resource_usage_metering(
        created,
        account,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        },
    )
    .unwrap();
    let window = open_resource_usage_window(&reconstructing, permit(&entry).await)
        .await
        .unwrap();

    let reconstructing = materialize_initial_files(reconstructing, prepared)
        .await
        .unwrap();
    assert_eq!(std::fs::read(root.join("read-only")).unwrap(), read_only);
    assert_eq!(std::fs::read(root.join("read-write")).unwrap(), read_write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            std::fs::metadata(root.join("read-only"))
                .unwrap()
                .permissions()
                .mode()
                & 0o222,
            0
        );
        assert_ne!(
            std::fs::metadata(root.join("read-write"))
                .unwrap()
                .permissions()
                .mode()
                & 0o200,
            0
        );
    }

    let reconstructing = finish_replay(reconstructing).await.unwrap();
    let resident = finish_reconstruction(reconstructing).await.unwrap();
    let generation_handle = resident_generation_handle(&resident);
    let entity_file = InitialAgentFile {
        content_hash: read_only_hash,
        path: AgentFilePath::from_abs_str("/entity-provisioned").unwrap(),
        permissions: AgentFilePermissions::ReadOnly,
        size: read_only.len() as u64,
    };
    provision_initial_files(
        &generation_handle,
        Arc::clone(&loader),
        id.environment_id,
        vec![entity_file.clone()],
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(root.join("entity-provisioned")).unwrap(),
        read_only
    );
    provision_initial_files(
        &generation_handle,
        Arc::clone(&loader),
        id.environment_id,
        vec![entity_file.clone()],
    )
    .unwrap()
    .await
    .unwrap();
    let conflict = provision_initial_files(
        &generation_handle,
        Arc::clone(&loader),
        id.environment_id,
        vec![InitialAgentFile {
            content_hash: read_write_hash,
            path: entity_file.path.clone(),
            permissions: AgentFilePermissions::ReadOnly,
            size: read_write.len() as u64,
        }],
    )
    .unwrap()
    .await
    .unwrap_err();
    assert!(
        conflict
            .to_string()
            .contains("conflicting owner filesystem provision declarations")
    );
    let replacement = InitialAgentFile {
        content_hash: read_only_hash,
        path: AgentFilePath::from_abs_str("/replacement-read-only").unwrap(),
        permissions: AgentFilePermissions::ReadOnly,
        size: read_only.len() as u64,
    };
    update_initial_files(
        &generation_handle,
        Arc::clone(&loader),
        id.environment_id,
        vec![files[1].clone(), replacement.clone()],
    )
    .unwrap()
    .await
    .unwrap();
    assert!(!root.join("read-only").exists());
    assert_eq!(
        std::fs::read(root.join("replacement-read-only")).unwrap(),
        read_only
    );
    assert_eq!(
        std::fs::read(root.join("entity-provisioned")).unwrap(),
        read_only
    );
    assert_eq!(
        path_permissions(
            &generation_handle,
            std::path::Path::new("replacement-read-only")
        )
        .unwrap(),
        AgentFilePermissions::ReadOnly
    );
    assert_eq!(
        path_permissions(&generation_handle, std::path::Path::new("read-write")).unwrap(),
        AgentFilePermissions::ReadWrite
    );
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    drop(loader);
    delete(seal(resident)).await.unwrap();
    assert!(!root.exists());
}

#[test]
fn initial_file_sandbox_task_failure_invalidates_the_generation() {
    let registry = GenerationRegistry::new();
    let failure = FilesystemStorageError::scripted_task_failure("update initial files");

    record_initial_file_update_failure(&registry, &failure);

    assert!(registry.is_invalidated());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("60s")]
async fn managed_xfs_lifecycle_installs_limits_and_deletes_verified() {
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let profile = FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root),
        ..FilesystemStorageConfig::default()
    };
    let initial_limits = limits(128 * 1024 * 1024, 8192);
    let provisioning = sandbox_provisioning(&profile).unwrap();
    let created = create_fresh(
        provisioning,
        agent_id(),
        ResolvedStorageLimits::Finite(initial_limits),
    )
    .await
    .unwrap();
    let (account, _) = account();
    let reconstructing = bind_resource_usage_metering(created, account).unwrap();
    let reconstructing = materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstructing = finish_replay(reconstructing).await.unwrap();
    let resident = finish_reconstruction(reconstructing).await.unwrap();
    let lowered_limits = limits(64 * 1024 * 1024, 4096);
    let resident = match set_limits(resident, ResolvedStorageLimits::Finite(lowered_limits))
        .await
        .unwrap()
    {
        LimitTransition::Resident(resident) => resident,
        LimitTransition::MustUnload(filesystem) => {
            delete(filesystem).await.unwrap();
            panic!("fresh managed filesystem exceeded its lowered limits")
        }
    };
    delete(seal(resident)).await.unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("60s")]
async fn managed_xfs_allocated_bytes_flow_through_resource_billing() {
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let profile = FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root),
        ..FilesystemStorageConfig::default()
    };
    let provisioning = sandbox_provisioning(&profile).unwrap();
    let created = create_fresh(
        provisioning,
        agent_id(),
        ResolvedStorageLimits::Finite(limits(16 * 1024 * 1024, 1024)),
    )
    .await
    .unwrap();
    let (account, entry) = account();
    let reconstructing = bind_resource_usage_metering(created, account).unwrap();
    let reconstructing = materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstructing = finish_replay(reconstructing).await.unwrap();
    let resident = finish_reconstruction(reconstructing).await.unwrap();
    let window = open_resource_usage_window(&resident, permit(&entry).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&resident);
    let opened = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "billing-allocation").unwrap(),
        OpenOptions::File {
            access: AccessMode::ReadWrite,
            disposition: FileDisposition::CreateOrTruncate,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap();
    let OpenNode::File(file) = opened.node else {
        panic!("managed XFS file open returned a directory")
    };
    let payload = Bytes::from(vec![0xa5; 128 * 1024]);
    assert_eq!(
        write(&generation_handle, &file, WritePlacement::At(0), payload)
            .unwrap()
            .await
            .unwrap(),
        WriteResult {
            written: 128 * 1024
        }
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    close_window(window, Instant::now() + Duration::from_secs(5))
        .await
        .unwrap();

    assert!(
        entry.durable_byte_seconds_delta() > 0,
        "authoritative managed-XFS allocation produced no durable storage charge"
    );
    release(OpenNode::File(file)).await.unwrap();
    delete(seal(resident)).await.unwrap();
}

#[test]
async fn unlimited_unmanaged_usage_opens_a_memory_only_window() {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    let filesystem = seal(filesystem);
    control.push_delete_and_verify(Ok(()));
    delete(filesystem).await.unwrap();
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        1
    );
}

#[test]
#[timeout("5s")]
async fn timed_out_billing_observer_does_not_block_sandbox_deletion() {
    let (provisioning, control) = ScriptedSandboxFilesystemProvisioning::new();
    let created = create_fresh_with_recovery::<ScriptedSandboxFilesystem>(
        provisioning,
        agent_id(),
        ResolvedStorageLimits::Unlimited,
        None,
    )
    .await
    .unwrap();
    let (account, entry) = account();
    let reconstructing = bind_resource_usage_metering(created, account).unwrap();
    let reconstructing = materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
        .await
        .unwrap();
    let reconstructing = finish_replay(reconstructing).await.unwrap();
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(reconstructing).await.unwrap();
    control.push_observe_allocation(Ok(FilesystemAllocation {
        allocated_bytes: 50,
        filesystem_objects: 1,
    }));
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count()
            < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    control.push_observe_allocation(Ok(FilesystemAllocation {
        allocated_bytes: 100,
        filesystem_objects: 1,
    }));
    let observation = control.block("observe_allocation");
    let filesystem = seal(filesystem);
    close_window(window, Instant::now() + Duration::from_millis(20))
        .await
        .unwrap();
    observation.wait_started().await;

    control.push_delete_and_verify(Ok(()));
    let deletion = control.block("delete_and_verify");
    let delete = tokio::spawn(delete(filesystem));
    deletion.wait_started().await;
    deletion.release();
    delete.await.unwrap().unwrap();
    observation.release();
}

#[test]
async fn partial_write_retries_only_the_unwritten_suffix() {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 13).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(3)));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(11),
            Bytes::from_static(b"hello")
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 5 }
    );
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert!(writes[0].contains("At(11)") && writes[0].contains("bytes=b\"hello\""));
    assert!(writes[1].contains("At(13)") && writes[1].contains("bytes=b\"llo\""));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn failed_attempt_preserves_its_prefix_and_retries_only_its_suffix() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(10, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 33).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        2,
        sandbox_error("write", std::io::ErrorKind::WouldBlock),
    )));
    control.push_write(Ok(SandboxWriteAttempt::completed(3)));
    let observations_before = call_count(&control, "observe_allocation(");

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(7),
            Bytes::from_static(b"hello"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 5 }
    );
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert!(writes[0].contains("At(7)") && writes[0].contains("bytes=b\"hello\""));
    assert!(writes[1].contains("At(9)") && writes[1].contains("bytes=b\"llo\""));
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn zero_progress_transient_failure_retries_without_observing_usage() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(20, 2)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 34).await;
    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("observe_allocation("))
        .count();
    for _ in 0..3 {
        control.push_write(Ok(SandboxWriteAttempt::failed(
            0,
            sandbox_error("write", std::io::ErrorKind::WouldBlock),
        )));
    }

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"blocked"),
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        observations_before
    );
    assert!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "still-admitted").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        )
        .is_ok()
    );

    control.push_observe_allocation(Ok(allocation(20, 2)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn terminal_zero_progress_failure_invalidates_without_observing_usage() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(30, 3)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 35).await;
    let observations_before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("observe_allocation("))
        .count();
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::PermissionDenied),
    )));

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"terminal"),
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        observations_before
    );
    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"rejected"),
        ),
        Err(AccessError::Revoked)
    ));

    control.push_observe_allocation(Ok(allocation(30, 3)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropped_terminal_write_observer_notifies_the_resident_generation() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(30, 3)).await;
    let activity = filesystem_activity(&filesystem);
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 351).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error(
            "detached terminal write",
            std::io::ErrorKind::PermissionDenied,
        ),
    )));
    let write_gate = control.block("write");
    let observer = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"terminal"),
        )
        .unwrap(),
    );
    write_gate.wait_started().await;
    observer.abort();

    write_gate.release();
    activity.wait_for_terminal_failure().await;
    assert!(activity.has_terminal_failure());

    control.push_observe_allocation(Ok(allocation(30, 3)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn stale_terminal_notification_does_not_mark_a_replacement_generation() {
    let (old, old_control, _) = resident(Err(unsupported_allocation())).await;
    let old_activity = filesystem_activity(&old);
    let (replacement, replacement_control, _) = resident(Err(unsupported_allocation())).await;
    let replacement_activity = filesystem_activity(&replacement);

    old.generation.as_ref().unwrap().invalidate();
    old_activity.wait_for_terminal_failure().await;
    assert!(old_activity.has_terminal_failure());
    assert!(!replacement_activity.has_terminal_failure());
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            replacement_activity.wait_for_terminal_failure(),
        )
        .await
        .is_err()
    );

    old_control.push_delete_and_verify(Ok(()));
    delete(seal(old)).await.unwrap();
    replacement_control.push_delete_and_verify(Ok(()));
    delete(seal(replacement)).await.unwrap();
}

#[test]
async fn unknown_effect_after_an_observed_prefix_invalidates_without_replaying_it() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(40, 4)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 36).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_observe_allocation(Ok(allocation(42, 4)));
    control.push_write(Err(sandbox_error("write task", std::io::ErrorKind::Other)));

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(5),
            Bytes::from_static(b"hello"),
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert_eq!(writes.len(), 2);
    assert!(writes[0].contains("At(5)") && writes[0].contains("bytes=b\"hello\""));
    assert!(writes[1].contains("At(7)") && writes[1].contains("bytes=b\"llo\""));

    control.push_observe_allocation(Ok(allocation(42, 4)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn successful_write_progress_does_not_consume_a_queued_observation_failure() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(50, 5)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 37).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    let observations_before = call_count(&control, "observe_allocation(");
    control.push_observe_allocation(Err(sandbox_error(
        "observe allocation",
        std::io::ErrorKind::Other,
    )));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"data"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 4 }
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        2
    );
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropped_write_observer_keeps_call_lease_without_billing_close_coupling() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 38).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(4)));
    let write_gate = control.block("write");
    let observer = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"data"),
        )
        .unwrap(),
    );
    write_gate.wait_started().await;
    observer.abort();

    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    let close = tokio::spawn(close_window(
        window,
        Instant::now() + Duration::from_secs(1),
    ));
    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(close.is_finished());
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));

    close.await.unwrap().unwrap();
    write_gate.release();
    write_gate.wait_completed().await;
    deletion.await.unwrap().unwrap();
}

#[test]
#[timeout("5s")]
async fn call_lease_blocks_deletion_before_sandbox_work_starts() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 44).await;
    let SandboxNode::File(sandbox_file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let append_guard = sandbox_file.coordinate_append().await;
    control.push_write(Ok(SandboxWriteAttempt::completed(1)));
    let observer = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::Append,
            Bytes::from_static(b"x"),
        )
        .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!has_call(&control, "write("));
    observer.abort();

    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));

    drop(append_guard);
    deletion.await.unwrap().unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
#[timeout("5s")]
async fn unstarted_write_without_storage_metering_does_not_block_billing_close() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 42).await;
    let unstarted = write(
        &generation_handle,
        &file,
        WritePlacement::At(0),
        Bytes::from_static(b"not-started"),
    )
    .unwrap();
    let close = tokio::spawn(close_window(
        window,
        Instant::now() + Duration::from_secs(1),
    ));
    tokio::time::timeout(Duration::from_secs(1), close)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!has_call(&control, "write("));

    drop(unstarted);
    assert!(!has_call(&control, "write("));
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn append_guard_covers_prefix_classification_suffix_retry_and_completion() {
    let recovery = BlockingWriteRecovery::new();
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let (opened_file, opened_alias) = SandboxOpened::scripted_file_aliases(45, 46, 47);
    control.push_open(Ok(opened_file));
    control.push_open(Ok(opened_alias));
    let file = open_programmed_file(&generation_handle, AccessMode::ReadWrite, "file").await;
    let alias = open_programmed_file(&generation_handle, AccessMode::ReadWrite, "hard-link").await;
    let SandboxNode::File(sandbox_file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let sandbox_file = sandbox_file.clone();

    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(80, 1)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));

    let first_append = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::Append,
            Bytes::from_static(b"abcd"),
        )
        .unwrap(),
    );
    recovery.wait_started().await;
    let first_effect = sandbox_file.append_coordination_counts();
    assert_eq!(first_effect.lookups, 1);
    assert_eq!(first_effect.allocations, 1);
    assert_eq!(first_effect.lock_acquisitions, 1);
    let first_calls = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect::<Vec<_>>();
    assert_eq!(first_calls.len(), 2);
    assert!(first_calls[0].contains("file=file(45)"));
    assert!(first_calls[0].contains("bytes=b\"abcd\""));
    assert!(first_calls[1].contains("file=file(45)"));
    assert!(first_calls[1].contains("bytes=b\"cd\""));

    let alias_append = tokio::spawn(
        write(
            &generation_handle,
            &alias,
            WritePlacement::Append,
            Bytes::from_static(b"XY"),
        )
        .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while sandbox_file.append_coordination_counts().lookups < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let alias_waiting = sandbox_file.append_coordination_counts();
    assert_eq!(alias_waiting.lookups, 2);
    assert_eq!(alias_waiting.allocations, 1);
    assert_eq!(alias_waiting.lock_acquisitions, 1);
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        2
    );
    assert!(!alias_append.is_finished());

    recovery.release();
    assert_eq!(
        first_append.await.unwrap().unwrap(),
        WriteResult { written: 4 }
    );
    assert_eq!(
        alias_append.await.unwrap().unwrap(),
        WriteResult { written: 2 }
    );
    let writes = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 4);
    assert!(writes[2].contains("file=file(45)"));
    assert!(writes[2].contains("bytes=b\"cd\""));
    assert!(writes[3].contains("file=file(46)"));
    assert!(writes[3].contains("bytes=b\"XY\""));
    assert_eq!(control.programmed_append_contents(47), b"abcdXY");
    assert_eq!(recovery.calls.load(Ordering::Acquire), 1);
    let completed = sandbox_file.append_coordination_counts();
    assert_eq!(completed.lookups, 2);
    assert_eq!(completed.allocations, 1);
    assert_eq!(completed.lock_acquisitions, 2);

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    release(OpenNode::File(alias)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn append_coordination_does_not_block_positioned_writes() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 39).await;
    let SandboxNode::File(sandbox_file) = file.ownership.sandbox() else {
        unreachable!("file wrapper must contain a sandbox file")
    };
    let sandbox_file = sandbox_file.clone();
    let opened = sandbox_file.append_coordination_counts();
    assert_eq!(opened.lookups, 0);
    assert_eq!(opened.allocations, 0);
    assert_eq!(opened.lock_acquisitions, 0);
    control.push_write(Ok(SandboxWriteAttempt::completed(1)));
    control.push_write(Ok(SandboxWriteAttempt::completed(1)));
    control.push_write(Ok(SandboxWriteAttempt::completed(1)));
    let first_append_gate = control.block("write");

    let first_append = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::Append,
            Bytes::from_static(b"a"),
        )
        .unwrap(),
    );
    first_append_gate.wait_started().await;
    let first_started = sandbox_file.append_coordination_counts();
    assert_eq!(first_started.lookups, 1);
    assert_eq!(first_started.allocations, 1);
    assert_eq!(first_started.lock_acquisitions, 1);
    let second_append = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::Append,
            Bytes::from_static(b"b"),
        )
        .unwrap(),
    );
    let positioned = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(11),
            Bytes::from_static(b"c"),
        )
        .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count()
            < 2
            || sandbox_file.append_coordination_counts().lookups < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let in_flight: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert_eq!(in_flight.len(), 2);
    assert!(in_flight.iter().any(|call| call.contains("Append")));
    assert!(in_flight.iter().any(|call| call.contains("At(11)")));
    let coordinated = sandbox_file.append_coordination_counts();
    assert_eq!(coordinated.lookups, 2);
    assert_eq!(coordinated.allocations, 1);
    assert_eq!(coordinated.lock_acquisitions, 1);

    first_append_gate.release();
    first_append.await.unwrap().unwrap();
    second_append.await.unwrap().unwrap();
    positioned.await.unwrap().unwrap();
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert_eq!(writes.len(), 3);
    assert!(writes[2].contains("Append"));
    let completed = sandbox_file.append_coordination_counts();
    assert_eq!(completed.lookups, 2);
    assert_eq!(completed.allocations, 1);
    assert_eq!(completed.lock_acquisitions, 2);
    assert_eq!(completed.live, 0);

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn append_coordination_is_independent_for_unrelated_files() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let first_file = open_file(&generation_handle, &control, 40).await;
    let second_file = open_file(&generation_handle, &control, 41).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(1)));
    control.push_write(Ok(SandboxWriteAttempt::completed(1)));
    let first_gate = control.block("write");

    let first = tokio::spawn(
        write(
            &generation_handle,
            &first_file,
            WritePlacement::Append,
            Bytes::from_static(b"a"),
        )
        .unwrap(),
    );
    first_gate.wait_started().await;
    let second = tokio::spawn(
        write(
            &generation_handle,
            &second_file,
            WritePlacement::Append,
            Bytes::from_static(b"b"),
        )
        .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count()
            < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    first_gate.release();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    control.push_release(Ok(()));
    release(OpenNode::File(first_file)).await.unwrap();
    release(OpenNode::File(second_file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn zero_progress_write_attempt_uses_a_bounded_no_effect_retry() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 28).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(0)));
    control.push_write(Ok(SandboxWriteAttempt::completed(4)));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(3),
            Bytes::from_static(b"data"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 4 }
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        2
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn successful_short_writes_advance_until_the_full_buffer_is_written() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 31).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(10),
            Bytes::from_static(b"abcdefgh"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 8 }
    );
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert_eq!(writes.len(), 4);
    assert!(writes[0].contains("At(10)") && writes[0].contains("bytes=b\"abcdefgh\""));
    assert!(writes[1].contains("At(12)") && writes[1].contains("bytes=b\"cdefgh\""));
    assert!(writes[2].contains("At(14)") && writes[2].contains("bytes=b\"efgh\""));
    assert!(writes[3].contains("At(16)") && writes[3].contains("bytes=b\"gh\""));
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "after-known-prefix").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("known write prefix invalidated filesystem generation handle");
    drop(admitted);

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn no_effect_failure_after_prefix_returns_the_prefix_when_budget_is_exhausted() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 32).await;
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    control.push_write(Ok(SandboxWriteAttempt::completed(2)));
    for _ in 0..3 {
        control.push_write(Ok(SandboxWriteAttempt::failed(
            0,
            sandbox_error("write", std::io::ErrorKind::WouldBlock),
        )));
    }

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(20),
            Bytes::from_static(b"abcdefgh"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 4 }
    );
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert_eq!(writes.len(), 5);
    assert!(writes[0].contains("At(20)") && writes[0].contains("bytes=b\"abcdefgh\""));
    assert!(writes[1].contains("At(22)") && writes[1].contains("bytes=b\"cdefgh\""));
    assert!(writes[2].contains("At(24)") && writes[2].contains("bytes=b\"efgh\""));
    assert!(writes[3].contains("At(24)") && writes[3].contains("bytes=b\"efgh\""));
    assert!(writes[4].contains("At(24)") && writes[4].contains("bytes=b\"efgh\""));
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "after-prefix-failure").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("proven no-effect suffix failure invalidated filesystem generation handle");
    drop(admitted);

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unknown_write_effect_invalidates_instead_of_retrying() {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 17).await;
    control.push_write(Err(sandbox_error("write", std::io::ErrorKind::Other)));

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"uncertain")
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));
    assert!(matches!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "after-invalidation").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        ),
        Err(AccessError::Revoked)
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        1
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unmanaged_storage_exhaustion_returns_without_retry_or_invalidation() {
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Unlimited).await;
    control.push_observe_allocation(Err(unsupported_allocation()));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    control.push_observe_allocation(Err(unsupported_allocation()));
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 18).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Err(unsupported_allocation()));

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"full")
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        1
    );
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "still-valid").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("unmanaged storage exhaustion invalidated the generation");
    drop(admitted);

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn storage_exhaustion_uses_quota_facts_without_guessing_physical_pressure() {
    let finite = limits(100, 10);
    let (filesystem, control, entry) = reconstructing(ResolvedStorageLimits::Finite(finite)).await;
    control.push_observe_allocation(Ok(allocation(10, 1)));
    let filesystem = finish_reconstruction(filesystem).await.unwrap();
    let window = open_resource_usage_window(&filesystem, permit(&entry).await)
        .await
        .unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 29).await;

    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(100, 1)));
    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"quota"),
        )
        .unwrap()
        .await,
        Err(Error::AgentQuota(_))
    ));

    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(50, 1)));
    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"pressure"),
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        2
    );
    let admitted = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "still-valid").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        },
    )
    .expect("classified storage exhaustion invalidated the generation");
    drop(admitted);

    control.push_observe_allocation(Ok(allocation(50, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn quota_failure_after_progress_returns_the_prefix_without_pressure_retry() {
    let finite = limits(100, 10);
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Finite(finite), allocation(90, 1))
            .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 43).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        2,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(100, 1)));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(4),
            Bytes::from_static(b"quota"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 2 }
    );
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        1
    );

    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::QuotaExceeded),
    )));
    control.push_observe_allocation(Ok(allocation(100, 1)));
    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(6),
            Bytes::from_static(b"ota"),
        )
        .unwrap()
        .await,
        Err(Error::AgentQuota(_))
    ));
    assert!(
        open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "quota-keeps-generation-valid").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            },
        )
        .is_ok()
    );

    control.push_observe_allocation(Ok(allocation(100, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn quota_exhaustion_never_requests_physical_recovery() {
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(90, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 45).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(100, 1)));

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"quota"),
        )
        .unwrap()
        .await,
        Err(Error::AgentQuota(_))
    ));
    assert_eq!(recovery.calls(), 0);

    control.push_observe_allocation(Ok(allocation(100, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn proven_physical_pressure_recovers_once_and_retries_only_the_suffix() {
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(90, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 46).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        2,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(92, 1)));
    control.push_write(Ok(SandboxWriteAttempt::completed(3)));
    control.push_observe_allocation(Ok(allocation(95, 1)));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(7),
            Bytes::from_static(b"hello"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 5 }
    );
    assert_eq!(recovery.calls(), 1);
    let writes: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("write("))
        .collect();
    assert_eq!(writes.len(), 2);
    assert!(writes[0].contains("At(7)") && writes[0].contains("bytes=b\"hello\""));
    assert!(writes[1].contains("At(9)") && writes[1].contains("bytes=b\"llo\""));

    control.push_observe_allocation(Ok(allocation(95, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn unavailable_pressure_authority_does_not_guess_or_retry() {
    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Unavailable]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 47).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(80, 1)));

    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"full"),
        )
        .unwrap()
        .await,
        Err(Error::Sandbox(_))
    ));
    assert_eq!(recovery.calls(), 1);
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        1
    );

    control.push_observe_allocation(Ok(allocation(80, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn denied_physical_recovery_returns_prefix_or_capacity_failure() {
    let recovery = ScriptedWriteRecovery::new([
        FilesystemWriteRecoveryOutcome::Denied,
        FilesystemWriteRecoveryOutcome::Denied,
    ]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 48).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        2,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(82, 1)));

    assert_eq!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"prefix"),
        )
        .unwrap()
        .await
        .unwrap(),
        WriteResult { written: 2 }
    );

    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(82, 1)));
    assert!(matches!(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(2),
            Bytes::from_static(b"efix"),
        )
        .unwrap()
        .await,
        Err(Error::PhysicalCapacity(_))
    ));
    assert_eq!(recovery.calls(), 2);

    control.push_observe_allocation(Ok(allocation(82, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropped_write_observer_keeps_recovery_without_billing_close_coupling() {
    let recovery = BlockingWriteRecovery::new();
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 49).await;
    control.push_write(Ok(SandboxWriteAttempt::failed(
        0,
        sandbox_error("write", std::io::ErrorKind::StorageFull),
    )));
    control.push_observe_allocation(Ok(allocation(80, 1)));
    control.push_write(Ok(SandboxWriteAttempt::completed(4)));
    let observer = tokio::spawn(
        write(
            &generation_handle,
            &file,
            WritePlacement::At(0),
            Bytes::from_static(b"data"),
        )
        .unwrap(),
    );
    recovery.wait_started().await;
    observer.abort();

    control.push_delete_and_verify(Ok(()));
    let close = tokio::spawn(close_window(
        window,
        Instant::now() + Duration::from_secs(1),
    ));
    let deletion = tokio::spawn(delete(seal(filesystem)));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(close.is_finished());
    assert!(!deletion.is_finished());
    assert!(!has_call(&control, "delete_and_verify("));

    close.await.unwrap().unwrap();
    recovery.release();
    control.push_release(Ok(()));
    release(OpenNode::File(file)).await.unwrap();
    deletion.await.unwrap().unwrap();
    assert_eq!(recovery.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("write("))
            .count(),
        2
    );
}

#[test]
async fn dropping_delete_observer_keeps_verified_deletion_module_owned() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file(&generation_handle, &control, 19).await;
    control.push_release(Ok(()));
    control.push_delete_and_verify(Ok(()));
    let release_gate = control.block("release");
    drop(release(OpenNode::File(file)));
    release_gate.wait_started().await;

    drop(delete(seal(filesystem)));
    assert!(!has_call(&control, "delete_and_verify("));
    release_gate.release();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !has_call(&control, "delete_and_verify(") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
async fn delete_observer_waits_for_sandbox_verification() {
    let (filesystem, control, _) = resident(Err(unsupported_allocation())).await;
    control.push_delete_and_verify(Ok(()));
    let verification_gate = control.block("delete_and_verify");
    let deletion = tokio::spawn(delete(seal(filesystem)));
    verification_gate.wait_started().await;

    assert!(!deletion.is_finished());
    verification_gate.release();
    deletion.await.unwrap().unwrap();
}

#[test]
async fn read_only_attribute_targets_are_rejected_before_sandbox_work() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = open_file_with_access(&generation_handle, &control, 60, AccessMode::Read).await;
    let node = OpenNode::File(file);
    let directory =
        open_directory_with_access(&generation_handle, &control, 61, AccessMode::Read).await;

    assert_eq!(
        set_attributes(
            &generation_handle,
            Target::Open(&node),
            AttributeChanges::File {
                size: 12,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            },
        )
        .unwrap_err(),
        AccessError::NotPermitted
    );
    assert!(!has_call(&control, "set_size("));
    assert!(!has_call(&control, "set_times("));
    let path = PathTarget::at(&directory, "child");
    assert_eq!(
        set_attributes(
            &generation_handle,
            Target::Path(&path, Follow::Yes),
            AttributeChanges::Times(TimeChanges {
                accessed: TimeChange::Now,
                modified: TimeChange::Keep,
            }),
        )
        .unwrap_err(),
        AccessError::NotPermitted
    );
    assert!(!has_call(&control, "set_times("));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    control.push_release(Ok(()));
    release(node).await.unwrap();
    release(OpenNode::Directory(directory)).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn successful_set_size_does_not_observe_allocation() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(10, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 61).await);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 4,
        accessed: None,
        modified: None,
    }));
    control.push_set_size(Ok(()));
    let observations_before = call_count(&control, "observe_allocation(");

    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::File {
            size: 12,
            times: TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Keep,
            },
        },
    )
    .unwrap()
    .await
    .unwrap();

    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn resize_postconditions_accept_desired_retry_no_effect_and_invalidate_unknown() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 62).await);
    let attributes = |size| SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size,
        accessed: None,
        modified: None,
    };
    control.push_get_attributes(Ok(attributes(4)));
    control.push_set_size(Err(sandbox_error("set size", std::io::ErrorKind::Other)));
    control.push_get_attributes(Ok(attributes(12)));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::File {
            size: 12,
            times: TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Keep,
            },
        },
    )
    .unwrap()
    .await
    .unwrap();

    control.push_get_attributes(Ok(attributes(12)));
    control.push_set_size(Err(sandbox_error(
        "set size",
        std::io::ErrorKind::WouldBlock,
    )));
    control.push_get_attributes(Ok(attributes(12)));
    control.push_set_size(Ok(()));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::File {
            size: 20,
            times: TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Keep,
            },
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("set_size("))
            .count(),
        3
    );

    control.push_get_attributes(Ok(attributes(20)));
    control.push_set_size(Err(sandbox_error("set size", std::io::ErrorKind::Other)));
    control.push_get_attributes(Ok(attributes(21)));
    assert!(matches!(
        set_attributes(
            &generation_handle,
            Target::Open(&node),
            AttributeChanges::File {
                size: 30,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            },
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn replay_time_restoration_accepts_a_read_only_descriptor() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(
        open_file_with_access(&generation_handle, &control, 66, AccessMode::Read).await,
    );
    let timestamp = std::time::UNIX_EPOCH + Duration::from_secs(40);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: None,
        modified: None,
    }));
    control.push_set_times(Ok(()));

    restore_times(
        &generation_handle,
        Target::Open(&node),
        TimeChanges {
            accessed: TimeChange::Set(timestamp),
            modified: TimeChange::Keep,
        },
    )
    .unwrap()
    .await
    .unwrap();

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn timestamp_postconditions_preserve_keep_and_retry_only_proven_no_effect() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 63).await);
    let old_accessed = std::time::UNIX_EPOCH + Duration::from_secs(10);
    let old_modified = std::time::UNIX_EPOCH + Duration::from_secs(20);
    let requested = std::time::UNIX_EPOCH + Duration::from_secs(30);
    let attributes = |accessed, modified| SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed,
        modified,
    };
    control.push_get_attributes(Ok(attributes(Some(old_accessed), Some(old_modified))));
    control.push_set_times(Err(sandbox_error(
        "set times",
        std::io::ErrorKind::WouldBlock,
    )));
    control.push_get_attributes(Ok(attributes(Some(old_accessed), Some(old_modified))));
    control.push_set_times(Ok(()));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::Times(TimeChanges {
            accessed: TimeChange::Keep,
            modified: TimeChange::Set(requested),
        }),
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("set_node_times("))
            .count(),
        2
    );

    control.push_get_attributes(Ok(attributes(Some(old_accessed), Some(requested))));
    control.push_set_times(Err(sandbox_error("set times", std::io::ErrorKind::Other)));
    control.push_get_attributes(Ok(attributes(Some(requested), Some(requested))));
    assert!(matches!(
        set_attributes(
            &generation_handle,
            Target::Open(&node),
            AttributeChanges::Times(TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Set(old_modified),
            }),
        )
        .unwrap()
        .await,
        Err(Error::RuntimeInvalidated)
    ));

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn successful_set_times_never_observes_allocation() {
    let timestamp = std::time::UNIX_EPOCH + Duration::from_secs(40);
    let attributes = SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: None,
        modified: None,
    };
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(40, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 64).await);
    let observations_before = call_count(&control, "observe_allocation(");
    control.push_get_attributes(Ok(attributes.clone()));
    control.push_set_times(Ok(()));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::Times(TimeChanges {
            accessed: TimeChange::Set(timestamp),
            modified: TimeChange::Keep,
        }),
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();

    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 65).await);
    let before = control
        .calls()
        .iter()
        .filter(|call| call.starts_with("observe_allocation("))
        .count();
    control.push_get_attributes(Ok(attributes));
    control.push_set_times(Ok(()));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::Times(TimeChanges {
            accessed: TimeChange::Set(timestamp),
            modified: TimeChange::Keep,
        }),
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        control
            .calls()
            .iter()
            .filter(|call| call.starts_with("observe_allocation("))
            .count(),
        before
    );
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn successful_resize_ignores_unrelated_observer_failure() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(50, 1)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 66).await);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 4,
        accessed: None,
        modified: None,
    }));
    control.push_set_size(Ok(()));
    let observations_before = call_count(&control, "observe_allocation(");
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::File {
            size: 8,
            times: TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Keep,
            },
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn resize_quota_precedes_pressure_and_growth_can_use_proven_recovery() {
    let quota_recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(90, 1),
        Some(quota_recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 67).await);
    let attributes = |size| SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size,
        accessed: None,
        modified: None,
    };
    control.push_get_attributes(Ok(attributes(10)));
    control.push_set_size(Err(sandbox_error(
        "set size",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_get_attributes(Ok(attributes(10)));
    control.push_observe_allocation(Ok(allocation(100, 1)));
    assert!(matches!(
        set_attributes(
            &generation_handle,
            Target::Open(&node),
            AttributeChanges::File {
                size: 20,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            },
        )
        .unwrap()
        .await,
        Err(Error::AgentQuota(_))
    ));
    assert_eq!(quota_recovery.calls(), 0);
    control.push_observe_allocation(Ok(allocation(100, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();

    let recovery = ScriptedWriteRecovery::new([FilesystemWriteRecoveryOutcome::Recovered]);
    let (filesystem, control, window) = authoritative_metered_resident_with_recovery(
        ResolvedStorageLimits::Finite(limits(100, 10)),
        allocation(80, 1),
        Some(recovery.handle()),
    )
    .await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 68).await);
    control.push_get_attributes(Ok(attributes(10)));
    control.push_set_size(Err(sandbox_error(
        "set size",
        std::io::ErrorKind::StorageFull,
    )));
    control.push_get_attributes(Ok(attributes(10)));
    control.push_observe_allocation(Ok(allocation(80, 1)));
    control.push_set_size(Ok(()));
    control.push_observe_allocation(Ok(allocation(90, 1)));
    set_attributes(
        &generation_handle,
        Target::Open(&node),
        AttributeChanges::File {
            size: 20,
            times: TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Keep,
            },
        },
    )
    .unwrap()
    .await
    .unwrap();
    assert_eq!(recovery.calls(), 1);
    control.push_observe_allocation(Ok(allocation(90, 1)));
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn read_only_file_and_directory_synchronization_skips_billing_observation() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(70, 2)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let file = OpenNode::File(
        open_file_with_access(&generation_handle, &control, 69, AccessMode::Read).await,
    );
    let directory = OpenNode::Directory(
        open_directory_with_access(&generation_handle, &control, 70, AccessMode::Read).await,
    );
    let observations_before = call_count(&control, "observe_allocation(");
    control.push_synchronize(Ok(()));
    synchronize(&generation_handle, &file, Synchronization::Data)
        .unwrap()
        .await
        .unwrap();
    control.push_synchronize(Ok(()));
    synchronize(
        &generation_handle,
        &directory,
        Synchronization::DataAndMetadata,
    )
    .unwrap()
    .await
    .unwrap();
    let synchronizations: Vec<_> = control
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("synchronize("))
        .collect();
    assert!(synchronizations[0].contains("level=Data)"));
    assert!(synchronizations[1].contains("level=DataAndMetadata)"));
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    control.push_release(Ok(()));
    release(file).await.unwrap();
    release(directory).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
async fn successful_synchronization_is_independent_from_storage_observer_failure() {
    let (filesystem, control, window) =
        authoritative_metered_resident(ResolvedStorageLimits::Unlimited, allocation(73, 2)).await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 73).await);
    control.push_synchronize(Ok(()));
    let observations_before = call_count(&control, "observe_allocation(");
    synchronize(&generation_handle, &node, Synchronization::Data)
        .unwrap()
        .await
        .unwrap();
    assert_eq!(
        call_count(&control, "observe_allocation("),
        observations_before
    );

    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn dropped_attribute_and_sync_observers_need_no_billing_close_coupling() {
    let (filesystem, control, window) = metered_resident().await;
    let generation_handle = resident_generation_handle(&filesystem);
    let node = OpenNode::File(open_file(&generation_handle, &control, 71).await);
    control.push_get_attributes(Ok(SandboxAttributes {
        kind: SandboxObjectKind::File,
        link_count: 1,
        size: 1,
        accessed: None,
        modified: None,
    }));
    control.push_set_size(Ok(()));
    let resize_gate = control.block("set_size");
    let observer = tokio::spawn(
        set_attributes(
            &generation_handle,
            Target::Open(&node),
            AttributeChanges::File {
                size: 2,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            },
        )
        .unwrap(),
    );
    resize_gate.wait_started().await;
    observer.abort();
    let close = tokio::spawn(close_window(
        window,
        Instant::now() + Duration::from_secs(1),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(close.is_finished());
    close.await.unwrap().unwrap();
    resize_gate.release();
    resize_gate.wait_completed().await;

    let (filesystem2, control2, window2) = metered_resident().await;
    let generation_handle2 = resident_generation_handle(&filesystem2);
    let node2 = OpenNode::File(
        open_file_with_access(&generation_handle2, &control2, 72, AccessMode::Read).await,
    );
    control2.push_synchronize(Ok(()));
    let sync_gate = control2.block("synchronize");
    let observer = tokio::spawn(
        synchronize(
            &generation_handle2,
            &node2,
            Synchronization::DataAndMetadata,
        )
        .unwrap(),
    );
    sync_gate.wait_started().await;
    observer.abort();
    let close2 = tokio::spawn(close_window(
        window2,
        Instant::now() + Duration::from_secs(1),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(close2.is_finished());
    close2.await.unwrap().unwrap();
    sync_gate.release();
    sync_gate.wait_completed().await;

    control.push_release(Ok(()));
    release(node).await.unwrap();
    control.push_delete_and_verify(Ok(()));
    delete(seal(filesystem)).await.unwrap();
    control2.push_release(Ok(()));
    release(node2).await.unwrap();
    control2.push_delete_and_verify(Ok(()));
    delete(seal(filesystem2)).await.unwrap();
}

#[test]
fn cause_effect_decision_keeps_quota_pressure_and_postconditions_distinct() {
    let storage_full = sandbox_error("write", std::io::ErrorKind::StorageFull);
    assert_eq!(
        classify_failure(
            &storage_full,
            FailureFacts {
                quota_exhausted: true,
                physical_capacity_exhausted: true,
            }
        ),
        FailureCause::AgentQuota
    );
    assert_eq!(
        classify_failure(
            &storage_full,
            FailureFacts {
                quota_exhausted: false,
                physical_capacity_exhausted: true,
            }
        ),
        FailureCause::PhysicalCapacity
    );
    assert_eq!(
        decide_effect(
            FailureCause::TransientBackend,
            EffectEvidence::DesiredPostconditionSatisfied,
            RetryBudget::new(0)
        ),
        EffectDecision::Succeed
    );
    assert_eq!(
        decide_effect(
            FailureCause::TransientBackend,
            EffectEvidence::CompletedPrefix(NonZeroU64::new(2).unwrap()),
            RetryBudget::new(1)
        ),
        EffectDecision::RetryUnwrittenSuffix
    );
    assert_eq!(
        decide_effect(
            FailureCause::AgentQuota,
            EffectEvidence::NoEffect,
            RetryBudget::new(2),
        ),
        EffectDecision::ReturnFailure(FailureCause::AgentQuota)
    );
    assert_eq!(
        decide_effect(
            FailureCause::PhysicalCapacity,
            EffectEvidence::NoEffect,
            RetryBudget::new(1),
        ),
        EffectDecision::ReclaimCapacityThenRetry
    );
    assert_eq!(
        decide_effect(
            FailureCause::TerminalInfrastructure,
            EffectEvidence::CompletedPrefix(NonZeroU64::new(2).unwrap()),
            RetryBudget::new(2),
        ),
        EffectDecision::Invalidate
    );
}

#[test]
fn namespace_pre_postcondition_evidence_requires_the_expected_transition() {
    let absent_insert = InsertPostcondition {
        path: NamespacePathState::Absent,
        desired: false,
    };
    let desired_insert = InsertPostcondition {
        path: NamespacePathState::Present(sandbox_attributes(SandboxObjectKind::Directory)),
        desired: true,
    };
    assert_eq!(
        insert_postcondition_evidence(&absent_insert, &desired_insert),
        EffectEvidence::DesiredPostconditionSatisfied
    );
    assert_eq!(
        insert_postcondition_evidence(&desired_insert, &desired_insert),
        EffectEvidence::NoEffect
    );

    let absent = NamespacePathState::Absent;
    let present = NamespacePathState::Present(sandbox_attributes(SandboxObjectKind::File));
    assert_eq!(
        hard_link_postcondition_evidence(&absent, &present),
        EffectEvidence::Unknown {
            known_completed_prefix: 0
        }
    );
    assert_eq!(
        hard_link_postcondition_evidence(&present, &present),
        EffectEvidence::NoEffect
    );
    assert_eq!(
        remove_postcondition_evidence(&absent, &absent, ObjectKind::File),
        EffectEvidence::NoEffect
    );
    assert_eq!(
        remove_postcondition_evidence(&present, &absent, ObjectKind::File),
        EffectEvidence::DesiredPostconditionSatisfied
    );
}

proptest! {
    #[test]
    fn limit_transition_requires_unload_exactly_when_allocation_exceeds_a_limit(
        byte_limit in 0_u64..u64::MAX,
        object_limit in 0_u64..u64::MAX,
        byte_relation in 0_u8..3,
        object_relation in 0_u8..3,
    ) {
        let level = |limit: u64, relation| match relation {
            0 => limit.saturating_sub(1),
            1 => limit,
            2 => limit + 1,
            _ => unreachable!(),
        };
        let allocated_bytes = level(byte_limit, byte_relation);
        let filesystem_objects = level(object_limit, object_relation);
        let expected = if allocated_bytes > byte_limit || filesystem_objects > object_limit {
            LimitDecision::MustUnload
        } else {
            LimitDecision::Resident
        };
        prop_assert_eq!(
            decide_limit_transition(
                allocation(allocated_bytes, filesystem_objects),
                limits(byte_limit, object_limit),
            ),
            expected
        );
    }

    #[test]
    fn move_postcondition_decision_uses_only_decisive_namespace_facts(
        source_existed in any::<bool>(),
        source_exists_after in any::<bool>(),
        destination_exists_after in any::<bool>(),
    ) {
        let state = |present| if present {
            NamespacePathState::Present(sandbox_attributes(SandboxObjectKind::File))
        } else {
            NamespacePathState::Absent
        };
        let expected = if source_existed && !source_exists_after && destination_exists_after {
            EffectEvidence::DesiredPostconditionSatisfied
        } else if !source_existed || source_exists_after {
            EffectEvidence::NoEffect
        } else {
            EffectEvidence::Unknown { known_completed_prefix: 0 }
        };

        prop_assert_eq!(
            move_postcondition_evidence(
                &state(source_existed),
                &state(source_exists_after),
                &state(destination_exists_after),
            ),
            expected
        );
    }

    #[test]
    fn resize_postcondition_decision_is_exact(
        before in any::<u64>(),
        requested in any::<u64>(),
        observed in any::<u64>(),
    ) {
        let expected = if observed == requested {
            EffectEvidence::DesiredPostconditionSatisfied
        } else if observed == before {
            EffectEvidence::NoEffect
        } else {
            EffectEvidence::Unknown { known_completed_prefix: 0 }
        };
        prop_assert_eq!(
            resize_postcondition_evidence(before, observed, requested),
            expected
        );
    }

    #[test]
    fn explicit_timestamp_postcondition_requires_set_values_and_preserved_keep(
        before_accessed in any::<u32>(),
        before_modified in any::<u32>(),
        requested_modified in any::<u32>(),
        observed_accessed in any::<u32>(),
        observed_modified in any::<u32>(),
    ) {
        let time = |seconds| Some(std::time::UNIX_EPOCH + Duration::from_secs(u64::from(seconds)));
        let before = SandboxAttributes {
            kind: SandboxObjectKind::File,
            link_count: 1,
            size: 0,
            accessed: time(before_accessed),
            modified: time(before_modified),
        };
        let observed = SandboxAttributes {
            accessed: time(observed_accessed),
            modified: time(observed_modified),
            ..before.clone()
        };
        let requested = TimeChanges {
            accessed: TimeChange::Keep,
            modified: TimeChange::Set(time(requested_modified).unwrap()),
        };
        let expected = if observed_accessed == before_accessed
            && observed_modified == requested_modified
        {
            EffectEvidence::DesiredPostconditionSatisfied
        } else if observed_accessed == before_accessed
            && observed_modified == before_modified
        {
            EffectEvidence::NoEffect
        } else {
            EffectEvidence::Unknown { known_completed_prefix: 0 }
        };
        prop_assert_eq!(
            timestamp_postcondition_evidence(
                &before,
                &observed,
                requested,
                (
                    std::time::UNIX_EPOCH,
                    std::time::UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX)),
                ),
            ),
            expected
        );
    }

    #[test]
    fn unknown_effect_always_invalidates(prefix in any::<u64>(), retries in any::<u8>()) {
        prop_assert_eq!(
            decide_effect(
                FailureCause::Guest,
                EffectEvidence::Unknown { known_completed_prefix: prefix },
                RetryBudget::new(retries),
            ),
            EffectDecision::Invalidate
        );
    }

    #[test]
    fn write_prefix_partition_and_position_are_exact(
        bytes in prop::collection::vec(any::<u8>(), 0..256),
        initial in 0_u64..u64::MAX / 2,
        completed_seed in any::<usize>(),
    ) {
        let completed = completed_seed % (bytes.len() + 1);
        let bytes = Bytes::from(bytes);
        let (placement, suffix) = unwritten_write_suffix(
            WritePlacement::At(initial),
            &bytes,
            completed as u64,
        )
        .unwrap();

        prop_assert_eq!(suffix.as_ref(), &bytes[completed..]);
        prop_assert_eq!(
            placement,
            SandboxWritePlacement::At(initial + completed as u64)
        );
        prop_assert_eq!(completed + suffix.len(), bytes.len());
    }

    #[test]
    fn advancing_completed_prefix_never_reintroduces_written_bytes(
        bytes in prop::collection::vec(any::<u8>(), 1..256),
        first_seed in any::<usize>(),
        second_seed in any::<usize>(),
    ) {
        let first = first_seed % (bytes.len() + 1);
        let second = first + second_seed % (bytes.len() - first + 1);
        let bytes = Bytes::from(bytes);
        let (_, first_suffix) = unwritten_write_suffix(WritePlacement::Append, &bytes, first as u64).unwrap();
        let (_, second_suffix) = unwritten_write_suffix(WritePlacement::Append, &bytes, second as u64).unwrap();

        prop_assert_eq!(first_suffix.as_ref(), &bytes[first..]);
        prop_assert_eq!(second_suffix.as_ref(), &bytes[second..]);
        prop_assert!(second_suffix.len() <= first_suffix.len());
    }
}
