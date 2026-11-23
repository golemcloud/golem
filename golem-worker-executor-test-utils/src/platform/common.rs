use super::*;

/// Starts a worker executor with a per-agent plan-level storage limit.
///
/// Managed-XFS variants install this limit as an authoritative project quota.
pub async fn start_with_agent_storage_quota(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes,
        }),
        TestExecutorOverrides::default(),
        None,
        "Timeout waiting for agent-storage-quota server to start",
    )
    .await
}
