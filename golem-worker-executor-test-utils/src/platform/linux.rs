pub use super::common::start_with_agent_storage_quota;
use super::native::start_with_filesystem_snapshots_on_unaccounted_storage;
use super::*;
use golem_worker_executor::services::golem_config::FilesystemStorageMode;
use golem_worker_executor::services::golem_config::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig,
};

struct MutableFilesystemStorageQuotaResourceLimits {
    entry: Arc<AtomicResourceEntry>,
}

#[async_trait]
impl ResourceLimits for MutableFilesystemStorageQuotaResourceLimits {
    async fn initialize_account(
        &self,
        _account_id: golem_common::model::account::AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(Arc::clone(&self.entry))
    }
}

/// Starts an executor on managed XFS that takes a snapshot after each invocation, with the
/// filesystem snapshot settings `filesystem_snapshots`.
pub async fn start_with_filesystem_snapshots_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
    filesystem_snapshots: golem_worker_executor::services::golem_config::FilesystemSnapshotsConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    let managed_xfs_root: Box<Path> = managed_xfs_root.into_boxed_path();
    // The test process runs no separate xfs-quotad: it holds CAP_SYS_ADMIN itself and serves the
    // quota calls on a thread.
    let xfs_quotad_socket: Box<Path> =
        golem_xfs_quotad::start_in_process(&managed_xfs_root)?.into_boxed_path();
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes,
        }),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.mode = FilesystemStorageMode::ManagedXfs {
                    root: managed_xfs_root.clone(),
                    xfs_quotad_socket: xfs_quotad_socket.clone(),
                };
                config.filesystem_snapshots = filesystem_snapshots.clone();
                config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
                config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        },
        None,
        "Timeout waiting for managed filesystem snapshot server to start",
    )
    .await
}

/// The per-agent disk limit of the default plan, 1 GiB.
const DEFAULT_PLAN_DISK_SPACE: u64 = 1024 * 1024 * 1024;

/// Starts an executor on XFS storage with reflink and without project quotas, at
/// `reflink_xfs_root`, that takes a snapshot after each invocation, with the filesystem snapshot
/// settings `filesystem_snapshots`.
///
/// The fixture leaves per-agent disk space unlimited because this storage cannot enforce a
/// finite limit. It meters no filesystem usage.
pub async fn start_with_filesystem_snapshots_on_reflink_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    reflink_xfs_root: PathBuf,
    filesystem_snapshots: golem_worker_executor::services::golem_config::FilesystemSnapshotsConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_filesystem_snapshots_on_unaccounted_storage(
        deps,
        context,
        FilesystemStorageMode::ReflinkXfs {
            root: reflink_xfs_root.into_boxed_path(),
        },
        filesystem_snapshots,
        "Timeout waiting for reflink XFS filesystem snapshot server to start",
    )
    .await
}

/// Starts an executor on XFS storage with reflink and without project quotas, at
/// `reflink_xfs_root`, with all resource usage metering on, filesystem metering included.
pub async fn start_with_filesystem_metering_on_reflink_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    reflink_xfs_root: PathBuf,
) -> anyhow::Result<TestWorkerExecutor> {
    let reflink_xfs_root: Box<Path> = reflink_xfs_root.into_boxed_path();
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes: DEFAULT_PLAN_DISK_SPACE,
        }),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.mode = FilesystemStorageMode::ReflinkXfs {
                    root: reflink_xfs_root.clone(),
                };
                config.resource_usage_metering = ResourceUsageMeteringConfig::all_enabled();
            })),
            ..Default::default()
        },
        None,
        "Timeout waiting for reflink XFS metering server to start",
    )
    .await
}

pub async fn start_with_agent_storage_quota_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_agent_storage_quota_and_pressure_on_managed_xfs(
        deps,
        context,
        max_disk_space_bytes,
        managed_xfs_root,
        FilesystemPressureConfig::default(),
    )
    .await
}

pub async fn start_with_agent_storage_quota_without_metering_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_agent_storage_quota_and_pressure_and_metering_on_managed_xfs(
        deps,
        context,
        max_disk_space_bytes,
        managed_xfs_root,
        FilesystemPressureConfig::default(),
        ResourceUsageMeteringConfig::default(),
    )
    .await
}

pub async fn start_with_agent_storage_quota_and_pressure_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
    pressure: FilesystemPressureConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_agent_storage_quota_and_pressure_and_metering_on_managed_xfs(
        deps,
        context,
        max_disk_space_bytes,
        managed_xfs_root,
        pressure,
        ResourceUsageMeteringConfig::all_enabled(),
    )
    .await
}

pub async fn start_with_agent_storage_quota_and_pressure_without_metering_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
    pressure: FilesystemPressureConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_agent_storage_quota_and_pressure_and_metering_on_managed_xfs(
        deps,
        context,
        max_disk_space_bytes,
        managed_xfs_root,
        pressure,
        ResourceUsageMeteringConfig::default(),
    )
    .await
}

/// Starts an executor on XFS storage with reflink and without project quotas, at
/// `reflink_xfs_root`, with the pressure settings `pressure` and without resource usage metering.
/// The fixture leaves per-agent disk space unlimited so volume pressure, not an unenforceable
/// quota, controls the test.
pub async fn start_with_pressure_on_reflink_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    reflink_xfs_root: PathBuf,
    pressure: FilesystemPressureConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    let reflink_xfs_root: Box<Path> = reflink_xfs_root.into_boxed_path();
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes: u64::MAX,
        }),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.mode = FilesystemStorageMode::ReflinkXfs {
                    root: reflink_xfs_root.clone(),
                };
                config.filesystem_storage.pressure = pressure.clone();
                config.resource_usage_metering = ResourceUsageMeteringConfig::default();
                config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
                config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        },
        None,
        "Timeout waiting for reflink XFS pressure server to start",
    )
    .await
}

async fn start_with_agent_storage_quota_and_pressure_and_metering_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
    pressure: FilesystemPressureConfig,
    metering: ResourceUsageMeteringConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    let managed_xfs_root: Box<Path> = managed_xfs_root.into_boxed_path();
    // The test process runs no separate xfs-quotad: it holds CAP_SYS_ADMIN itself and serves the
    // quota calls on a thread.
    let xfs_quotad_socket: Box<Path> =
        golem_xfs_quotad::start_in_process(&managed_xfs_root)?.into_boxed_path();
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes,
        }),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.mode = FilesystemStorageMode::ManagedXfs {
                    root: managed_xfs_root.clone(),
                    xfs_quotad_socket: xfs_quotad_socket.clone(),
                };
                config.filesystem_storage.pressure = pressure.clone();
                config.resource_usage_metering = metering;
                config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
                config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        },
        None,
        "Timeout waiting for managed agent-storage-quota server to start",
    )
    .await
}

pub async fn start_with_mutable_agent_storage_quota_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
) -> anyhow::Result<(TestWorkerExecutor, MutableFilesystemStorageQuota)> {
    start_with_mutable_agent_storage_quota_and_metering_on_managed_xfs(
        deps,
        context,
        max_disk_space_bytes,
        managed_xfs_root,
        ResourceUsageMeteringConfig::all_enabled(),
    )
    .await
}

pub async fn start_with_mutable_agent_storage_quota_without_metering_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
) -> anyhow::Result<(TestWorkerExecutor, MutableFilesystemStorageQuota)> {
    start_with_mutable_agent_storage_quota_and_metering_on_managed_xfs(
        deps,
        context,
        max_disk_space_bytes,
        managed_xfs_root,
        ResourceUsageMeteringConfig::default(),
    )
    .await
}

async fn start_with_mutable_agent_storage_quota_and_metering_on_managed_xfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
    managed_xfs_root: PathBuf,
    metering: ResourceUsageMeteringConfig,
) -> anyhow::Result<(TestWorkerExecutor, MutableFilesystemStorageQuota)> {
    let entry = Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        max_disk_space_bytes,
        u64::MAX,
    ));
    let managed_xfs_root: Box<Path> = managed_xfs_root.into_boxed_path();
    // The test process runs no separate xfs-quotad: it holds CAP_SYS_ADMIN itself and serves the
    // quota calls on a thread.
    let xfs_quotad_socket: Box<Path> =
        golem_xfs_quotad::start_in_process(&managed_xfs_root)?.into_boxed_path();
    let executor = run_production_context_bootstrap(
        deps,
        context,
        Arc::new(MutableFilesystemStorageQuotaResourceLimits {
            entry: Arc::clone(&entry),
        }),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.mode = FilesystemStorageMode::ManagedXfs {
                    root: managed_xfs_root.clone(),
                    xfs_quotad_socket: xfs_quotad_socket.clone(),
                };
                config.resource_usage_metering = metering;
                config.filesystem_storage.filesystem_object_limit_policy =
                    FilesystemObjectLimitPolicyConfig::new(262_144, 1, 1024).unwrap();
            })),
            ..Default::default()
        },
        None,
        "Timeout waiting for mutable managed agent-storage-quota server to start",
    )
    .await?;
    Ok((executor, MutableFilesystemStorageQuota { entry }))
}
