use super::*;
use anyhow::ensure;

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
        Resource::Storage => ensure!(updates.iter().any(|u| u.monthly_policy_revision == revision
            && u.period == period
            && if durable {
                u.durable_storage_byte_seconds_delta == 1
            } else {
                u.ephemeral_storage_byte_seconds_delta == 1
            })),
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

pub(super) fn available_monthly_policy() -> MonthlyResourcePolicy {
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
