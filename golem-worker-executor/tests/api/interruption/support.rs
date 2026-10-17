use super::*;
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
            available_durable_storage_byte_seconds: 1,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 1,
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
