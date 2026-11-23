pub use super::common::start_with_agent_storage_quota;
use super::native::start_with_filesystem_snapshots_on_unaccounted_storage;
use super::*;
use golem_worker_executor::services::golem_config::FilesystemStorageMode;

/// Starts an executor on APFS development storage that takes a snapshot after each invocation.
/// The fixture leaves per-agent disk space unlimited because this storage has no accounting or
/// quota enforcement. It meters no filesystem usage.
pub async fn start_with_filesystem_snapshots_on_apfs(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    root: Box<Path>,
    filesystem_snapshots: golem_worker_executor::services::golem_config::FilesystemSnapshotsConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_filesystem_snapshots_on_unaccounted_storage(
        deps,
        context,
        FilesystemStorageMode::Apfs { root },
        filesystem_snapshots,
        "Timeout waiting for APFS filesystem snapshot server to start",
    )
    .await
}
