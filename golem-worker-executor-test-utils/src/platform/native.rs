use super::*;

use golem_worker_executor::services::golem_config::FilesystemStorageMode;

pub(super) async fn start_with_filesystem_snapshots_on_unaccounted_storage(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    mode: FilesystemStorageMode,
    filesystem_snapshots: golem_worker_executor::services::golem_config::FilesystemSnapshotsConfig,
    timeout_message: &'static str,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes: u64::MAX,
        }),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.filesystem_storage.mode = mode.clone();
                config.resource_usage_metering.filesystem = false;
                config.filesystem_snapshots = filesystem_snapshots.clone();
                config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
                config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        },
        None,
        timeout_message,
    )
    .await
}
