use super::*;
use anyhow::Context as _;
use anyhow::ensure;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::default::Context as WorkerContext;

#[derive(Debug)]
pub(super) struct ObservedUsageFlusher {
    pub(super) owner: std::sync::Weak<Worker<WorkerContext>>,
    pub(super) send: tokio::sync::mpsc::UnboundedSender<(u64, u64)>,
}

impl ResourceUsageFlusher for ObservedUsageFlusher {
    fn flush_usage(&self) {
        if let Some(owner) = self.owner.upgrade()
            && owner.monthly_horizon_was_registered()
            && owner.monthly_window_active_for_test()
        {
            let current = owner.current_monthly_proposal_for_test();
            let _ = self
                .send
                .send((current.diagnostic_window_id, current.policy_revision));
        }
    }
}

// Observe the ordinary execution-status receipt while the test holds real provider or oplog
// work. There is no separate publication transaction or private receipt in Worker.
pub(super) async fn published_interrupt_receipt<
    Ctx: golem_worker_executor::workerctx::WorkerCtx,
>(
    worker: &golem_worker_executor::worker::Worker<Ctx>,
) -> anyhow::Result<tokio::sync::broadcast::Receiver<()>> {
    let kind =
        tokio::time::timeout(Duration::from_secs(10), worker.await_interrupt_for_test()).await?;
    let resources = worker.owner_runtime_resources();
    let status = resources.execution_status();
    let status = status.read().unwrap();
    match &*status {
        golem_worker_executor::model::ExecutionStatus::Interrupting {
            interrupt_kind,
            await_interruption,
            ..
        } if *interrupt_kind == kind => Ok(await_interruption.subscribe()),
        status => {
            anyhow::bail!("interruption receipt is not pending while test work is held: {status:?}")
        }
    }
}

pub(super) const FUEL_BUDGET: u64 = 1_000_000_000;

pub(super) fn managed_filesystem_config(
    metering: ResourceUsageMeteringConfig,
) -> anyhow::Result<Option<golem_worker_executor::services::golem_config::FilesystemStorageMode>> {
    if !metering.filesystem {
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    {
        use golem_worker_executor::services::golem_config::FilesystemStorageMode;
        let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
            })?;
        let socket = golem_xfs_quotad::start_in_process(&root)?;
        Ok(Some(FilesystemStorageMode::ManagedXfs {
            root: root.into_boxed_path(),
            xfs_quotad_socket: socket.into_boxed_path(),
        }))
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("ManagedXfs storage metering requires Linux")
}

fn managed_runtime_path(owned: &OwnedAgentId) -> anyhow::Result<std::path::PathBuf> {
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("GOLEM_MANAGED_XFS_TEST_ROOT is not set"))?;
    Ok(root
        .join(owned.environment_id.to_string())
        .join(owned.agent_id.component_id.to_string())
        .join(golem_service_base::storage::blob::agent_path_segment(
            &owned.agent_id,
        )))
}

// Write through the installed project quota, without a guest checkpoint that could trigger
// quota enforcement itself. The ordinary allocation sampler must stop the pending native wait.
pub(super) async fn allocate_storage(owned: &OwnedAgentId) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;

    let path = managed_runtime_path(owned)?.join("metered-allocation");
    let mut file = tokio::fs::File::create(path).await?;
    file.write_all(&vec![0x5a; 1024 * 1024]).await?;
    file.sync_all().await?;
    Ok(())
}

// Positive whole units at this healthy revision prove allocation billing while native work
// remains pending, not usage at the later exhausted revision: immediate close can add zero there.
pub(super) async fn allocate_storage_with_billing_refresh(
    owned: &OwnedAgentId,
    limits: &Arc<ResourceLimitsGrpc>,
    registry: &MutableResourceLimitsRegistry,
    policy: &MonthlyResourcePolicy,
    durable: bool,
) -> anyhow::Result<()> {
    allocate_storage(owned).await?;
    registry.set_policy(policy.clone());
    let revision = registry.current_limits().monthly_policy_revision;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            limits.run_batch_for_test().await;
            if registry.applied_updates().iter().any(|update| {
                update.monthly_policy_revision == revision
                    && update.period == policy.period
                    && if durable {
                        update.durable_storage_byte_seconds_delta > 0
                    } else {
                        update.ephemeral_storage_byte_seconds_delta > 0
                    }
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("positive managed XFS usage under the refreshed live policy")?;
    Ok(())
}

// Revision changes queue whole and fractional settlements separately. Deliver those old
// batches before taking the baseline for subsequent no-post-close-accrual assertions.
pub(super) async fn flush_stopped_usage(
    limits: &Arc<ResourceLimitsGrpc>,
    registry: &MutableResourceLimitsRegistry,
    resource: Resource,
) -> anyhow::Result<()> {
    if resource != Resource::Storage {
        limits.run_batch_for_test().await;
        return Ok(());
    }
    let revision = registry.current_limits().monthly_policy_revision;
    let delivered = registry.applied_updates().len();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            limits.run_batch_for_test().await;
            if registry.applied_updates()[delivered..]
                .iter()
                .any(|update| update.monthly_policy_revision == revision)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("delivery of stopped window settlements through the current policy revision")?;
    Ok(())
}

pub(super) fn assert_storage_deleted(owned: &OwnedAgentId) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(managed_runtime_path(owned)?);
    ensure!(
        matches!(&metadata, Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "storage generation was not deleted: {metadata:?}"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Resource {
    Memory,
    Compute,
    Storage,
}

impl Resource {
    pub(super) fn metering(self) -> ResourceUsageMeteringConfig {
        ResourceUsageMeteringConfig {
            compute: self == Self::Compute,
            memory: self == Self::Memory,
            filesystem: self == Self::Storage,
        }
    }

    pub(super) fn policy(self) -> MonthlyResourcePolicy {
        MonthlyResourcePolicy {
            period: AccountUsagePeriod::current(),
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: if self == Self::Compute {
                FUEL_BUDGET
            } else {
                0
            },
            available_memory_gb_seconds: if self == Self::Memory { u64::MAX } else { 0 },
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        }
    }

    pub(super) fn reason(self, durable: bool) -> &'static str {
        match self {
            Self::Memory => "monthly memory exhausted",
            Self::Compute => "monthly compute exhausted",
            Self::Storage if durable => "monthly durable storage exhausted",
            Self::Storage => "monthly ephemeral storage exhausted",
        }
    }
}

pub(super) fn check_accounting(
    updates: &[ResourceUsageUpdate],
    resource: Resource,
    durable: bool,
    revision: u64,
    period: AccountUsagePeriod,
) -> anyhow::Result<()> {
    match resource {
        Resource::Memory => ensure!(updates.iter().any(|u| u.monthly_policy_revision == revision
            && u.period == period
            && (u.memory_gb_seconds_delta > 0 || u.memory_byte_nanoseconds_remainder > 0))),
        Resource::Compute => {
            let fuel: i128 = updates.iter().map(|u| i128::from(u.fuel_delta)).sum();
            ensure!(
                fuel > 0 && fuel < i128::from(FUEL_BUDGET),
                "unused prepaid fuel must be refunded: {fuel}"
            );
        }
        Resource::Storage => ensure!(
            updates.iter().any(|u| u.monthly_policy_revision == revision
                && u.period == period
                && if durable {
                    u.durable_storage_byte_seconds_delta > 0
                } else {
                    u.ephemeral_storage_byte_seconds_delta > 0
                }),
            "missing storage usage for policy {revision} in {period}: {updates:?}"
        ),
    }
    for update in updates {
        if resource != Resource::Compute {
            ensure!(update.fuel_delta == 0);
        }
        if resource != Resource::Memory {
            ensure!(
                update.memory_gb_seconds_delta == 0
                    && update.memory_byte_nanoseconds_remainder == 0
            );
        }
        if resource != Resource::Storage || !durable {
            ensure!(
                update.durable_storage_byte_seconds_delta == 0
                    && update.durable_storage_byte_nanoseconds_remainder == 0
            );
        }
        if resource != Resource::Storage || durable {
            ensure!(
                update.ephemeral_storage_byte_seconds_delta == 0
                    && update.ephemeral_storage_byte_nanoseconds_remainder == 0
            );
        }
    }
    Ok(())
}

pub(super) fn available_quota_policy() -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}
