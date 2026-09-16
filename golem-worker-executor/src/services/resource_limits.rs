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

use crate::metrics::resources::{
    record_ephemeral_overdraft_fuel, record_fuel_borrow, record_fuel_return,
    record_memory_gb_seconds, record_resource_usage_batch_update_failure,
    record_storage_byte_seconds,
};
use crate::services::byte_time_accumulator::ByteTimeSettlement;
use crate::services::golem_config::{ResourceLimitsConfig, ResourceUsageMeteringConfig};
use async_trait::async_trait;
use chrono::Utc;
use golem_common::SafeDisplay;
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AccountUsagePeriod, BYTE_NANOSECONDS_PER_GB_SECOND, EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
    MonthlyUsageMode,
};
use golem_common::model::agent::AgentMode;
use golem_service_base::clients::registry::{RegistryService, ResourceUsageUpdate};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::MonthlyResourcePolicy;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::{Instrument, error, info_span};

#[derive(Debug)]
pub struct AtomicResourceEntry {
    metering: ResourceUsageMeteringConfig,
    usage_revision_state: Mutex<UsageRevisionState>,
    // any local fuel consumption that was not yet sent to the server
    delta: AtomicI64,
    // any fuel consumption that is currently in flight to the server
    in_flight_delta: AtomicI64,
    in_flight_memory_gb_seconds_delta: AtomicI64,
    in_flight_durable_memory_gb_seconds_delta: AtomicI64,
    in_flight_ephemeral_memory_gb_seconds_delta: AtomicI64,
    account_usage_accumulator: Option<Mutex<AccountUsageAccumulator>>,
    resource_usage_flushers: Mutex<Vec<Weak<dyn ResourceUsageFlusher>>>,
    agent_memory_limit_targets: Mutex<Vec<Weak<dyn AgentMemoryLimitTarget>>>,
    // Current (cached) value of the account level worker memory limits
    max_memory: AtomicUsize,
    // Current (cached) value of the account level worker function table element limits
    max_table_elements: AtomicUsize,
    // Current (cached) value of the account level per-worker disk space limit
    max_disk_space: AtomicU64,
    filesystem_limit_update: AsyncMutex<()>,
    agent_filesystem_limit_targets: scc::HashMap<OwnedAgentId, Weak<AgentFilesystemLimitTarget>>,
    // Unix timestamp (seconds) of the last time fuel/memory were refreshed from
    // the server. Used by the background loop to detect idle accounts whose
    // cached limits have grown stale (e.g. after a plan change or monthly reset).
    last_refresh_secs: AtomicI64,
    // Plan-level per-invocation HTTP call limit. Uses AtomicU64 so that it can
    // be updated when the account's plan changes (propagated via batch responses).
    per_invocation_http_call_limit: AtomicU64,
    // Plan-level per-invocation RPC call limit.
    per_invocation_rpc_call_limit: AtomicU64,

    // Monthly account-level HTTP call tracking.
    // The available count last reported by the registry service.
    available_http_calls_from_server: AtomicU64,
    // HTTP calls made locally since the last successful batch sync to the registry.
    unsynced_http_calls: AtomicU64,
    // HTTP calls included in the batch currently being sent; cleared on success or failure.
    syncing_http_calls: AtomicU64,

    // Monthly account-level RPC call tracking (same pattern as HTTP).
    available_rpc_calls_from_server: AtomicU64,
    unsynced_rpc_calls: AtomicU64,
    syncing_rpc_calls: AtomicU64,

    // Maximum number of concurrently running agents on a single executor for this
    // account. Uses the unlimited sentinel (10^18) when unlimited.
    // Refreshed via update_last_known_limits when batch sync responses arrive.
    max_concurrent_agents_per_executor: AtomicU64,

    // Plan-level per-agent oplog write rate limit (writes per second).
    // UNLIMITED_OPLOG_WRITES_PER_SECOND (10^18) means no rate limiting.
    // Refreshed via update_last_known_limits when batch sync responses arrive.
    oplog_writes_per_second: AtomicU64,
}

pub(crate) trait ResourceUsageFlusher: Send + Sync + std::fmt::Debug {
    fn flush_usage(&self);
}

pub(crate) trait AgentMemoryLimitTarget: Send + Sync + std::fmt::Debug {
    fn enforce_limit(&self, limit: u64);
}

type AgentFilesystemLimitUpdate =
    Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send + 'static>>;

struct AgentFilesystemLimitTarget {
    update: Arc<dyn Fn(u64) -> AgentFilesystemLimitUpdate + Send + Sync>,
}

impl std::fmt::Debug for AgentFilesystemLimitTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFilesystemLimitTarget")
            .finish_non_exhaustive()
    }
}

pub(crate) struct AgentFilesystemLimitRegistration {
    resource_limits: Arc<AtomicResourceEntry>,
    owned_agent_id: OwnedAgentId,
    target: Arc<AgentFilesystemLimitTarget>,
}

impl Drop for AgentFilesystemLimitRegistration {
    fn drop(&mut self) {
        self.resource_limits
            .agent_filesystem_limit_targets
            .remove_if_sync(&self.owned_agent_id, |registered| {
                registered
                    .upgrade()
                    .is_none_or(|registered| Arc::ptr_eq(&registered, &self.target))
            });
    }
}

#[derive(Clone, Copy, Debug)]
struct CapturedUsageUpdate {
    update: ResourceUsageUpdate,
    durable_memory_gb_seconds_delta: i64,
    ephemeral_memory_gb_seconds_delta: i64,
}

impl CapturedUsageUpdate {
    fn retryable_byte_time(mut self) -> Option<Self> {
        self.update.fuel_delta = 0;
        self.update.http_call_count_delta = 0;
        self.update.rpc_call_count_delta = 0;
        (!LocalMonthlyUsage::from_update(&self.update).is_zero()).then_some(self)
    }
}

#[derive(Debug)]
struct UsageRevisionState {
    current_policy_revision: u64,
    current_mode_revision: u64,
    current_period: AccountUsagePeriod,
    pending: VecDeque<CapturedUsageUpdate>,
    monthly_policy: Option<MonthlyPolicyGate>,
}

#[derive(Debug)]
struct MonthlyPolicyGate {
    period: AccountUsagePeriod,
    mode: MonthlyUsageMode,
    available_fuel: Option<u64>,
    available_memory_byte_nanoseconds: Option<u128>,
    available_durable_storage_byte_nanoseconds: Option<u128>,
    available_ephemeral_storage_byte_nanoseconds: Option<u128>,
    failed_delivery_usage: BTreeMap<AccountUsagePeriod, LocalMonthlyUsage>,
    unassigned_in_flight_usage: BTreeMap<AccountUsagePeriod, LocalMonthlyUsage>,
    in_flight_usage: HashMap<u64, InFlightMonthlyUsage>,
    stale_delivered_usage: Vec<StaleDeliveredUsage>,
    hard_limit_memory_overshoot_byte_nanoseconds: BTreeMap<(AccountUsagePeriod, u64), u128>,
    hard_limit_durable_storage_overshoot_byte_nanoseconds:
        BTreeMap<(AccountUsagePeriod, u64), u128>,
    hard_limit_ephemeral_storage_overshoot_byte_nanoseconds:
        BTreeMap<(AccountUsagePeriod, u64), u128>,
    refresh_generation: u64,
    settled_generation: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LocalMonthlyUsage {
    fuel: i128,
    memory_byte_nanoseconds: u128,
    durable_storage_byte_nanoseconds: u128,
    ephemeral_storage_byte_nanoseconds: u128,
}

impl LocalMonthlyUsage {
    fn from_update(update: &ResourceUsageUpdate) -> Self {
        Self {
            fuel: update.fuel_delta as i128,
            memory_byte_nanoseconds: (update.memory_gb_seconds_delta.max(0) as u128)
                .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                .saturating_add(update.memory_byte_nanoseconds_remainder as u128),
            durable_storage_byte_nanoseconds: (update.durable_storage_byte_seconds_delta.max(0)
                as u128)
                .saturating_mul(1_000_000_000)
                .saturating_add(update.durable_storage_byte_nanoseconds_remainder as u128),
            ephemeral_storage_byte_nanoseconds: (update.ephemeral_storage_byte_seconds_delta.max(0)
                as u128)
                .saturating_mul(1_000_000_000)
                .saturating_add(update.ephemeral_storage_byte_nanoseconds_remainder as u128),
        }
    }

    fn is_zero(self) -> bool {
        self.fuel == 0
            && self.memory_byte_nanoseconds == 0
            && self.durable_storage_byte_nanoseconds == 0
            && self.ephemeral_storage_byte_nanoseconds == 0
    }

    fn positive(self) -> Self {
        Self {
            fuel: self.fuel.max(0),
            memory_byte_nanoseconds: self.memory_byte_nanoseconds,
            durable_storage_byte_nanoseconds: self.durable_storage_byte_nanoseconds,
            ephemeral_storage_byte_nanoseconds: self.ephemeral_storage_byte_nanoseconds,
        }
    }

    fn saturating_add_assign(&mut self, other: Self) {
        self.fuel = self.fuel.saturating_add(other.fuel);
        self.memory_byte_nanoseconds = self
            .memory_byte_nanoseconds
            .saturating_add(other.memory_byte_nanoseconds);
        self.durable_storage_byte_nanoseconds = self
            .durable_storage_byte_nanoseconds
            .saturating_add(other.durable_storage_byte_nanoseconds);
        self.ephemeral_storage_byte_nanoseconds = self
            .ephemeral_storage_byte_nanoseconds
            .saturating_add(other.ephemeral_storage_byte_nanoseconds);
    }

    fn saturating_sub_assign(&mut self, other: Self) {
        self.fuel = self.fuel.saturating_sub(other.fuel);
        self.memory_byte_nanoseconds = self
            .memory_byte_nanoseconds
            .saturating_sub(other.memory_byte_nanoseconds);
        self.durable_storage_byte_nanoseconds = self
            .durable_storage_byte_nanoseconds
            .saturating_sub(other.durable_storage_byte_nanoseconds);
        self.ephemeral_storage_byte_nanoseconds = self
            .ephemeral_storage_byte_nanoseconds
            .saturating_sub(other.ephemeral_storage_byte_nanoseconds);
    }

    fn saturating_add_nonnegative_assign(&mut self, other: Self) {
        self.saturating_add_assign(other);
        self.fuel = self.fuel.max(0);
    }
}

#[derive(Clone, Copy, Debug)]
struct InFlightMonthlyUsage {
    captured: CapturedUsageUpdate,
    usage: LocalMonthlyUsage,
}

#[derive(Clone, Copy, Debug)]
struct StaleDeliveredUsage {
    period: AccountUsagePeriod,
    usage: LocalMonthlyUsage,
    observed_refresh_generation: u64,
}

fn retain_unaccepted_delivery(
    gate: &mut MonthlyPolicyGate,
    delivered: Option<InFlightMonthlyUsage>,
    current_period: AccountUsagePeriod,
    usage_update_applied: bool,
) -> Option<CapturedUsageUpdate> {
    let delivered = delivered?;
    if !usage_update_applied {
        if delivered.captured.update.period == gate.period
            || delivered.captured.update.period == current_period
        {
            let failed_discrete_usage = LocalMonthlyUsage {
                fuel: delivered.usage.fuel,
                ..LocalMonthlyUsage::default()
            };
            if !failed_discrete_usage.is_zero() {
                gate.failed_delivery_usage
                    .entry(delivered.captured.update.period)
                    .or_default()
                    .saturating_add_nonnegative_assign(failed_discrete_usage);
            }
        }
        return delivered.captured.retryable_byte_time();
    }
    if delivered.captured.update.period == gate.period
        || delivered.captured.update.period == current_period
    {
        let usage = delivered.usage.positive();
        if !usage.is_zero() {
            gate.stale_delivered_usage.push(StaleDeliveredUsage {
                period: delivered.captured.update.period,
                usage,
                observed_refresh_generation: gate.refresh_generation,
            });
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MonthlyResourceExhaustion {
    Compute,
    Memory,
    DurableStorage,
    EphemeralStorage,
}

impl MonthlyResourceExhaustion {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::Compute => "monthly compute exhausted",
            Self::Memory => "monthly memory exhausted",
            Self::DurableStorage => "monthly durable storage exhausted",
            Self::EphemeralStorage => "monthly ephemeral storage exhausted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FuelBorrow {
    Borrowed {
        amount: u64,
        revision: u64,
        generation: u64,
        period: AccountUsagePeriod,
    },
    Exhausted {
        revision: u64,
        generation: u64,
        period: AccountUsagePeriod,
    },
}

impl FuelBorrow {
    pub(crate) fn revision(self) -> u64 {
        match self {
            Self::Borrowed { revision, .. } | Self::Exhausted { revision, .. } => revision,
        }
    }

    pub(crate) fn period(self) -> AccountUsagePeriod {
        match self {
            Self::Borrowed { period, .. } | Self::Exhausted { period, .. } => period,
        }
    }
}

#[derive(Debug)]
/// Account-local consumption settled by resident meters but not yet captured for registry delivery.
///
/// These values are usage, not reservations. Whole units remain as `u128` until `capture` removes
/// a wire-sized batch; sub-unit byte-nanosecond remainders carry across short-lived agent windows.
struct AccountUsageAccumulator {
    memory: Option<MemoryUsageAccumulator>,
    storage: Option<StorageUsageAccumulator>,
}

#[derive(Debug, Default)]
struct MemoryUsageAccumulator {
    durable_memory_gb_seconds: u128,
    ephemeral_memory_gb_seconds: u128,
    remainder: u128,
}

#[derive(Debug, Default)]
struct StorageUsageAccumulator {
    durable_storage_byte_seconds: u128,
    ephemeral_storage_byte_seconds: u128,
    durable_storage_remainder: u128,
    ephemeral_storage_remainder: u128,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct CapturedAccountUsage {
    memory_gb_seconds: i64,
    durable_memory_gb_seconds: i64,
    ephemeral_memory_gb_seconds: i64,
    durable_storage_byte_seconds: i64,
    ephemeral_storage_byte_seconds: i64,
    memory_byte_nanoseconds_remainder: u64,
    durable_storage_byte_nanoseconds_remainder: u64,
    ephemeral_storage_byte_nanoseconds_remainder: u64,
}

impl CapturedAccountUsage {
    fn is_zero(&self) -> bool {
        self.memory_gb_seconds == 0
            && self.durable_storage_byte_seconds == 0
            && self.ephemeral_storage_byte_seconds == 0
            && self.memory_byte_nanoseconds_remainder == 0
            && self.durable_storage_byte_nanoseconds_remainder == 0
            && self.ephemeral_storage_byte_nanoseconds_remainder == 0
    }
}

impl AccountUsageAccumulator {
    fn new(config: ResourceUsageMeteringConfig) -> Self {
        Self {
            memory: config.memory.then(MemoryUsageAccumulator::default),
            storage: config.filesystem.then(StorageUsageAccumulator::default),
        }
    }

    fn add_storage(&mut self, mode: AgentMode, units: u128) {
        let Some(storage) = &mut self.storage else {
            return;
        };
        let pending = match mode {
            AgentMode::Durable => &mut storage.durable_storage_byte_seconds,
            AgentMode::Ephemeral => &mut storage.ephemeral_storage_byte_seconds,
        };
        *pending = pending.saturating_add(units);
    }

    fn add_memory_settlement(
        &mut self,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
        maximum_byte_nanoseconds: Option<u128>,
    ) -> u128 {
        let Some(memory) = &mut self.memory else {
            return 0;
        };
        let requested_byte_nanoseconds = settlement
            .units
            .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
            .saturating_add(settlement.remainder);
        let billable_byte_nanoseconds = maximum_byte_nanoseconds
            .map_or(requested_byte_nanoseconds, |maximum| {
                requested_byte_nanoseconds.min(maximum)
            });
        let billable_units = billable_byte_nanoseconds / BYTE_NANOSECONDS_PER_GB_SECOND;
        let billable_remainder = billable_byte_nanoseconds % BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder = memory.remainder.saturating_add(billable_remainder);
        let remainder_units = memory.remainder / BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder %= BYTE_NANOSECONDS_PER_GB_SECOND;
        let pending = match mode {
            AgentMode::Durable => &mut memory.durable_memory_gb_seconds,
            AgentMode::Ephemeral => &mut memory.ephemeral_memory_gb_seconds,
        };
        *pending = pending.saturating_add(billable_units.saturating_add(remainder_units));
        requested_byte_nanoseconds.saturating_sub(billable_byte_nanoseconds)
    }

    fn add_storage_settlement(
        &mut self,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
        maximum_byte_nanoseconds: Option<u128>,
    ) -> u128 {
        let Some(storage) = &mut self.storage else {
            return 0;
        };
        let requested_byte_nanoseconds = settlement
            .units
            .saturating_mul(1_000_000_000)
            .saturating_add(settlement.remainder);
        let billable_byte_nanoseconds = maximum_byte_nanoseconds
            .map_or(requested_byte_nanoseconds, |maximum| {
                requested_byte_nanoseconds.min(maximum)
            });
        let billable_units = billable_byte_nanoseconds / 1_000_000_000;
        let billable_remainder = billable_byte_nanoseconds % 1_000_000_000;
        let remainder = match mode {
            AgentMode::Durable => &mut storage.durable_storage_remainder,
            AgentMode::Ephemeral => &mut storage.ephemeral_storage_remainder,
        };
        *remainder = remainder.saturating_add(billable_remainder);
        let remainder_units = *remainder / 1_000_000_000;
        *remainder %= 1_000_000_000;
        self.add_storage(mode, billable_units.saturating_add(remainder_units));
        requested_byte_nanoseconds.saturating_sub(billable_byte_nanoseconds)
    }

    fn is_active(&self) -> bool {
        self.memory.as_ref().is_some_and(|memory| {
            memory.durable_memory_gb_seconds != 0 || memory.ephemeral_memory_gb_seconds != 0
        }) || self.storage.as_ref().is_some_and(|storage| {
            storage.durable_storage_byte_seconds != 0 || storage.ephemeral_storage_byte_seconds != 0
        })
    }

    #[cfg(test)]
    fn memory(&self, mode: AgentMode) -> u128 {
        self.memory.as_ref().map_or(0, |memory| match mode {
            AgentMode::Durable => memory.durable_memory_gb_seconds,
            AgentMode::Ephemeral => memory.ephemeral_memory_gb_seconds,
        })
    }

    fn total_memory(&self) -> u128 {
        self.memory.as_ref().map_or(0, |memory| {
            memory
                .durable_memory_gb_seconds
                .saturating_add(memory.ephemeral_memory_gb_seconds)
        })
    }

    fn memory_remainder(&self) -> u128 {
        self.memory.as_ref().map_or(0, |memory| memory.remainder)
    }

    fn storage(&self, mode: AgentMode) -> u128 {
        self.storage.as_ref().map_or(0, |storage| match mode {
            AgentMode::Durable => storage.durable_storage_byte_seconds,
            AgentMode::Ephemeral => storage.ephemeral_storage_byte_seconds,
        })
    }

    fn capture(&mut self, include_remainders: bool) -> CapturedAccountUsage {
        let (durable_memory, ephemeral_memory) = self.memory.as_mut().map_or((0, 0), |memory| {
            let durable = take_bounded(&mut memory.durable_memory_gb_seconds, i64::MAX as u128);
            let ephemeral = take_bounded(
                &mut memory.ephemeral_memory_gb_seconds,
                i64::MAX as u128 - durable,
            );
            (durable, ephemeral)
        });
        let (durable_storage, ephemeral_storage) =
            self.storage.as_mut().map_or((0, 0), |storage| {
                (
                    take_bounded(&mut storage.durable_storage_byte_seconds, i64::MAX as u128),
                    take_bounded(
                        &mut storage.ephemeral_storage_byte_seconds,
                        i64::MAX as u128,
                    ),
                )
            });
        CapturedAccountUsage {
            memory_gb_seconds: (durable_memory + ephemeral_memory) as i64,
            durable_memory_gb_seconds: durable_memory as i64,
            ephemeral_memory_gb_seconds: ephemeral_memory as i64,
            durable_storage_byte_seconds: durable_storage as i64,
            ephemeral_storage_byte_seconds: ephemeral_storage as i64,
            memory_byte_nanoseconds_remainder: self.memory.as_mut().map_or(0, |memory| {
                capture_remainder(&mut memory.remainder, include_remainders)
            }),
            durable_storage_byte_nanoseconds_remainder: self.storage.as_mut().map_or(
                0,
                |storage| {
                    capture_remainder(&mut storage.durable_storage_remainder, include_remainders)
                },
            ),
            ephemeral_storage_byte_nanoseconds_remainder: self.storage.as_mut().map_or(
                0,
                |storage| {
                    capture_remainder(&mut storage.ephemeral_storage_remainder, include_remainders)
                },
            ),
        }
    }
}

impl Default for AccountUsageAccumulator {
    fn default() -> Self {
        Self::new(ResourceUsageMeteringConfig::all_enabled())
    }
}

fn take_bounded(pending: &mut u128, maximum: u128) -> u128 {
    let captured = (*pending).min(maximum);
    *pending -= captured;
    captured
}

fn capture_remainder(remainder: &mut u128, capture: bool) -> u64 {
    if capture {
        std::mem::take(remainder) as u64
    } else {
        0
    }
}

fn monthly_policy_available_memory_byte_nanoseconds(policy: &MonthlyResourcePolicy) -> u128 {
    (policy.available_memory_gb_seconds as u128)
        .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
        .saturating_add(policy.available_memory_byte_nanoseconds_remainder as u128)
}

fn monthly_policy_available_storage_byte_nanoseconds(
    policy: &MonthlyResourcePolicy,
    mode: AgentMode,
) -> u128 {
    let (seconds, remainder) = match mode {
        AgentMode::Durable => (
            policy.available_durable_storage_byte_seconds,
            policy.available_durable_storage_byte_nanoseconds_remainder,
        ),
        AgentMode::Ephemeral => (
            policy.available_ephemeral_storage_byte_seconds,
            policy.available_ephemeral_storage_byte_nanoseconds_remainder,
        ),
    };
    (seconds as u128)
        .saturating_mul(1_000_000_000)
        .saturating_add(remainder as u128)
}

impl AtomicResourceEntry {
    /// Sentinel value used in the database and service config to represent
    /// "unlimited" for the concurrent agents per executor limit.
    /// `1_000_000_000_000_000_000` (10^18) — fits in i64 (TOML max) and
    /// is safe for SQLite REAL, consistent with `monthly_gas_limit` and the
    /// `default_unlimited()` convention from PR #3068.
    pub const UNLIMITED_CONCURRENT_AGENTS: u64 = 1_000_000_000_000_000_000;

    /// Sentinel value for the oplog write rate limit meaning "no rate limit".
    /// Same 10^18 value — fits in i64 (TOML max), safe for SQLite REAL,
    /// consistent with other unlimited sentinels in this codebase.
    pub const UNLIMITED_OPLOG_WRITES_PER_SECOND: u64 = 1_000_000_000_000_000_000;
    // This matches the existing registry plan sentinel and fits every supported database backend.
    pub(crate) const EFFECTIVELY_UNLIMITED_DISK_SPACE: u64 = EFFECTIVELY_UNLIMITED_STORAGE_LIMIT;

    pub fn new(
        fuel: u64,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        max_concurrent_agents_per_executor: u64,
    ) -> Self {
        Self::new_with_all_limits(
            fuel,
            max_memory,
            max_table_elements,
            max_disk_space,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            max_concurrent_agents_per_executor,
            Self::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        )
    }

    pub fn new_with_monthly_policy(
        monthly_policy: MonthlyResourcePolicy,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        max_concurrent_agents_per_executor: u64,
    ) -> Self {
        Self::new_with_all_limits_metering_policy_and_revisions(
            monthly_policy,
            max_memory,
            max_table_elements,
            max_disk_space,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            max_concurrent_agents_per_executor,
            Self::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig::all_enabled(),
            0,
            0,
            0,
        )
    }

    pub fn new_with_invocation_limits(
        fuel: u64,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
    ) -> Self {
        Self::new_with_all_limits(
            fuel,
            max_memory,
            max_table_elements,
            max_disk_space,
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
            u64::MAX,
            u64::MAX,
            Self::UNLIMITED_CONCURRENT_AGENTS,
            Self::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        )
    }

    /// Full constructor used when all limits (including monthly HTTP/RPC) are available
    /// from the registry at initialization time.
    pub fn new_with_all_limits(
        fuel: u64,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        available_http_calls: u64,
        available_rpc_calls: u64,
        max_concurrent_agents_per_executor: u64,
        oplog_writes_per_second: u64,
    ) -> Self {
        Self::new_with_all_limits_and_metering(
            fuel,
            max_memory,
            max_table_elements,
            max_disk_space,
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
            available_http_calls,
            available_rpc_calls,
            max_concurrent_agents_per_executor,
            oplog_writes_per_second,
            ResourceUsageMeteringConfig::all_enabled(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_all_limits_and_metering(
        fuel: u64,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        available_http_calls: u64,
        available_rpc_calls: u64,
        max_concurrent_agents_per_executor: u64,
        oplog_writes_per_second: u64,
        metering: ResourceUsageMeteringConfig,
    ) -> Self {
        Self::new_with_all_limits_metering_and_revision(
            fuel,
            max_memory,
            max_table_elements,
            max_disk_space,
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
            available_http_calls,
            available_rpc_calls,
            max_concurrent_agents_per_executor,
            oplog_writes_per_second,
            metering,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_all_limits_metering_and_revision(
        fuel: u64,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        available_http_calls: u64,
        available_rpc_calls: u64,
        max_concurrent_agents_per_executor: u64,
        oplog_writes_per_second: u64,
        metering: ResourceUsageMeteringConfig,
        monthly_usage_mode_revision: u64,
    ) -> Self {
        Self::new_with_all_limits_metering_policy_and_revisions(
            MonthlyResourcePolicy {
                period: AccountUsagePeriod::current(),
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: fuel,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            max_memory,
            max_table_elements,
            max_disk_space,
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
            available_http_calls,
            available_rpc_calls,
            max_concurrent_agents_per_executor,
            oplog_writes_per_second,
            metering,
            monthly_usage_mode_revision,
            monthly_usage_mode_revision,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_all_limits_metering_policy_and_revisions(
        monthly_policy: MonthlyResourcePolicy,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        per_invocation_http_call_limit: u64,
        per_invocation_rpc_call_limit: u64,
        available_http_calls: u64,
        available_rpc_calls: u64,
        max_concurrent_agents_per_executor: u64,
        oplog_writes_per_second: u64,
        metering: ResourceUsageMeteringConfig,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        refresh_generation: u64,
    ) -> Self {
        Self {
            metering,
            usage_revision_state: Mutex::new(UsageRevisionState {
                current_policy_revision: monthly_policy_revision,
                current_mode_revision: monthly_usage_mode_revision,
                current_period: monthly_policy.period,
                pending: VecDeque::new(),
                monthly_policy: (metering.compute || metering.memory || metering.filesystem)
                    .then_some(MonthlyPolicyGate {
                        period: monthly_policy.period,
                        mode: monthly_policy.mode,
                        available_fuel: metering.compute.then_some(monthly_policy.available_fuel),
                        available_memory_byte_nanoseconds: metering.memory.then(|| {
                            monthly_policy_available_memory_byte_nanoseconds(&monthly_policy)
                        }),
                        available_durable_storage_byte_nanoseconds: metering.filesystem.then(
                            || {
                                monthly_policy_available_storage_byte_nanoseconds(
                                    &monthly_policy,
                                    AgentMode::Durable,
                                )
                            },
                        ),
                        available_ephemeral_storage_byte_nanoseconds: metering.filesystem.then(
                            || {
                                monthly_policy_available_storage_byte_nanoseconds(
                                    &monthly_policy,
                                    AgentMode::Ephemeral,
                                )
                            },
                        ),
                        failed_delivery_usage: BTreeMap::new(),
                        unassigned_in_flight_usage: BTreeMap::new(),
                        in_flight_usage: HashMap::new(),
                        stale_delivered_usage: Vec::new(),
                        hard_limit_memory_overshoot_byte_nanoseconds: BTreeMap::new(),
                        hard_limit_durable_storage_overshoot_byte_nanoseconds: BTreeMap::new(),
                        hard_limit_ephemeral_storage_overshoot_byte_nanoseconds: BTreeMap::new(),
                        refresh_generation,
                        settled_generation: refresh_generation,
                    }),
            }),
            delta: AtomicI64::new(0),
            in_flight_delta: AtomicI64::new(0),
            in_flight_memory_gb_seconds_delta: AtomicI64::new(0),
            in_flight_durable_memory_gb_seconds_delta: AtomicI64::new(0),
            in_flight_ephemeral_memory_gb_seconds_delta: AtomicI64::new(0),
            account_usage_accumulator: metering
                .any_byte_time_enabled()
                .then(|| Mutex::new(AccountUsageAccumulator::new(metering))),
            resource_usage_flushers: Mutex::new(Vec::new()),
            agent_memory_limit_targets: Mutex::new(Vec::new()),
            max_memory: AtomicUsize::new(max_memory),
            max_table_elements: AtomicUsize::new(max_table_elements),
            max_disk_space: AtomicU64::new(max_disk_space),
            filesystem_limit_update: AsyncMutex::new(()),
            agent_filesystem_limit_targets: scc::HashMap::new(),
            last_refresh_secs: AtomicI64::new(Utc::now().timestamp()),
            per_invocation_http_call_limit: AtomicU64::new(per_invocation_http_call_limit),
            per_invocation_rpc_call_limit: AtomicU64::new(per_invocation_rpc_call_limit),
            available_http_calls_from_server: AtomicU64::new(available_http_calls),
            unsynced_http_calls: AtomicU64::new(0),
            syncing_http_calls: AtomicU64::new(0),
            available_rpc_calls_from_server: AtomicU64::new(available_rpc_calls),
            unsynced_rpc_calls: AtomicU64::new(0),
            syncing_rpc_calls: AtomicU64::new(0),
            max_concurrent_agents_per_executor: AtomicU64::new(max_concurrent_agents_per_executor),
            oplog_writes_per_second: AtomicU64::new(oplog_writes_per_second),
        }
    }

    pub fn per_invocation_http_call_limit(&self) -> u64 {
        self.per_invocation_http_call_limit.load(Ordering::Acquire)
    }

    pub fn per_invocation_rpc_call_limit(&self) -> u64 {
        self.per_invocation_rpc_call_limit.load(Ordering::Acquire)
    }

    pub fn oplog_writes_per_second(&self) -> u64 {
        self.oplog_writes_per_second.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn set_oplog_writes_per_second(&self, value: u64) {
        self.oplog_writes_per_second.store(value, Ordering::Release);
    }

    fn secs_since_last_refresh(&self) -> i64 {
        Utc::now()
            .timestamp()
            .saturating_sub(self.last_refresh_secs.load(Ordering::Acquire))
    }

    #[cfg(test)]
    fn effective_fuel(&self) -> u64 {
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.effective_fuel_with_revision_state(&revision_state)
    }

    fn effective_fuel_with_revision_state(&self, revision_state: &UsageRevisionState) -> u64 {
        let Some(gate) = &revision_state.monthly_policy else {
            return u64::MAX;
        };
        let Some(available_fuel) = gate.available_fuel else {
            return u64::MAX;
        };
        (available_fuel as i128)
            .saturating_sub(self.local_monthly_usage(revision_state).fuel)
            .clamp(0, u64::MAX as i128) as u64
    }

    #[cfg(test)]
    fn effective_memory_gb_seconds(&self) -> u64 {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.effective_memory_with_revision_state(&revision_state)
    }

    #[cfg(test)]
    fn effective_memory_with_revision_state(&self, revision_state: &UsageRevisionState) -> u64 {
        (self.effective_memory_byte_nanoseconds_with_revision_state(revision_state)
            / BYTE_NANOSECONDS_PER_GB_SECOND)
            .min(u64::MAX as u128) as u64
    }

    fn effective_memory_byte_nanoseconds_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> u128 {
        let Some(gate) = &revision_state.monthly_policy else {
            return (u64::MAX as u128).saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND);
        };
        let Some(available_memory) = gate.available_memory_byte_nanoseconds else {
            return (u64::MAX as u128).saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND);
        };
        available_memory.saturating_sub(
            self.local_monthly_usage(revision_state)
                .memory_byte_nanoseconds,
        )
    }

    fn memory_settlement_limit(&self, revision_state: &UsageRevisionState) -> Option<u128> {
        let gate = revision_state.monthly_policy.as_ref()?;
        (gate.period == revision_state.current_period
            && gate.mode == MonthlyUsageMode::HardLimit
            && gate.available_memory_byte_nanoseconds.is_some())
        .then(|| self.effective_memory_byte_nanoseconds_with_revision_state(revision_state))
    }

    fn effective_storage_byte_nanoseconds_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
        mode: AgentMode,
    ) -> u128 {
        let Some(gate) = &revision_state.monthly_policy else {
            return (u64::MAX as u128).saturating_mul(1_000_000_000);
        };
        let available = match mode {
            AgentMode::Durable => gate.available_durable_storage_byte_nanoseconds,
            AgentMode::Ephemeral => gate.available_ephemeral_storage_byte_nanoseconds,
        };
        let Some(available) = available else {
            return (u64::MAX as u128).saturating_mul(1_000_000_000);
        };
        let usage = self.local_monthly_usage(revision_state);
        available.saturating_sub(match mode {
            AgentMode::Durable => usage.durable_storage_byte_nanoseconds,
            AgentMode::Ephemeral => usage.ephemeral_storage_byte_nanoseconds,
        })
    }

    fn storage_settlement_limit(
        &self,
        revision_state: &UsageRevisionState,
        mode: AgentMode,
    ) -> Option<u128> {
        let gate = revision_state.monthly_policy.as_ref()?;
        let available = match mode {
            AgentMode::Durable => gate.available_durable_storage_byte_nanoseconds,
            AgentMode::Ephemeral => gate.available_ephemeral_storage_byte_nanoseconds,
        };
        (gate.period == revision_state.current_period
            && gate.mode == MonthlyUsageMode::HardLimit
            && available.is_some())
        .then(|| self.effective_storage_byte_nanoseconds_with_revision_state(revision_state, mode))
    }

    fn record_hard_limit_memory_overshoot(revision_state: &mut UsageRevisionState, amount: u128) {
        if amount == 0 {
            return;
        }
        let key = (
            revision_state.current_period,
            revision_state.current_policy_revision,
        );
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("memory usage was clipped without an active monthly policy");
        let overshoot = gate
            .hard_limit_memory_overshoot_byte_nanoseconds
            .entry(key)
            .or_default();
        *overshoot = overshoot.saturating_add(amount);
    }

    fn record_hard_limit_storage_overshoot(
        revision_state: &mut UsageRevisionState,
        mode: AgentMode,
        amount: u128,
    ) {
        if amount == 0 {
            return;
        }
        let key = (
            revision_state.current_period,
            revision_state.current_policy_revision,
        );
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("storage usage was clipped without an active monthly policy");
        let overshoots = match mode {
            AgentMode::Durable => &mut gate.hard_limit_durable_storage_overshoot_byte_nanoseconds,
            AgentMode::Ephemeral => {
                &mut gate.hard_limit_ephemeral_storage_overshoot_byte_nanoseconds
            }
        };
        let overshoot = overshoots.entry(key).or_default();
        *overshoot = overshoot.saturating_add(amount);
    }

    fn local_monthly_usage(&self, revision_state: &UsageRevisionState) -> LocalMonthlyUsage {
        let gate = revision_state
            .monthly_policy
            .as_ref()
            .expect("monthly policy gate is active");
        let period_matches = |period: AccountUsagePeriod| {
            period == gate.period || period == revision_state.current_period
        };
        let mut usage = LocalMonthlyUsage {
            fuel: self.delta.load(Ordering::Acquire) as i128,
            memory_byte_nanoseconds: self.account_usage_accumulator.as_ref().map_or(
                0,
                |accumulator| {
                    let accumulator = accumulator.lock().unwrap();
                    accumulator
                        .total_memory()
                        .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                        .saturating_add(accumulator.memory_remainder())
                },
            ),
            durable_storage_byte_nanoseconds: self.account_usage_accumulator.as_ref().map_or(
                0,
                |accumulator| {
                    let accumulator = accumulator.lock().unwrap();
                    accumulator
                        .storage(AgentMode::Durable)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(
                            accumulator
                                .storage
                                .as_ref()
                                .map_or(0, |storage| storage.durable_storage_remainder),
                        )
                },
            ),
            ephemeral_storage_byte_nanoseconds: self.account_usage_accumulator.as_ref().map_or(
                0,
                |accumulator| {
                    let accumulator = accumulator.lock().unwrap();
                    accumulator
                        .storage(AgentMode::Ephemeral)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(
                            accumulator
                                .storage
                                .as_ref()
                                .map_or(0, |storage| storage.ephemeral_storage_remainder),
                        )
                },
            ),
        };
        for in_flight in gate
            .in_flight_usage
            .values()
            .filter(|in_flight| period_matches(in_flight.captured.update.period))
        {
            usage.saturating_add_assign(in_flight.usage);
        }
        for local in gate
            .unassigned_in_flight_usage
            .iter()
            .filter(|(period, _)| period_matches(**period))
            .map(|(_, usage)| *usage)
        {
            usage.saturating_add_assign(local);
        }
        for local in revision_state
            .pending
            .iter()
            .filter(|captured| period_matches(captured.update.period))
            .map(|captured| LocalMonthlyUsage::from_update(&captured.update))
        {
            usage.saturating_add_assign(local);
        }
        for local in gate
            .stale_delivered_usage
            .iter()
            .filter(|delivered| period_matches(delivered.period))
            .map(|delivered| delivered.usage.positive())
        {
            usage.saturating_add_assign(local);
        }
        for local in gate
            .failed_delivery_usage
            .iter()
            .filter(|(period, _)| period_matches(**period))
            .map(|(_, usage)| *usage)
        {
            usage.saturating_add_assign(local);
        }
        let memory_overshoot = gate
            .hard_limit_memory_overshoot_byte_nanoseconds
            .iter()
            .filter(|((period, _), _)| period_matches(*period))
            .map(|(_, amount)| *amount)
            .fold(0u128, u128::saturating_add);
        usage.memory_byte_nanoseconds = usage
            .memory_byte_nanoseconds
            .saturating_add(memory_overshoot);
        let durable_storage_overshoot = gate
            .hard_limit_durable_storage_overshoot_byte_nanoseconds
            .iter()
            .filter(|((period, _), _)| period_matches(*period))
            .map(|(_, amount)| *amount)
            .fold(0u128, u128::saturating_add);
        usage.durable_storage_byte_nanoseconds = usage
            .durable_storage_byte_nanoseconds
            .saturating_add(durable_storage_overshoot);
        let ephemeral_storage_overshoot = gate
            .hard_limit_ephemeral_storage_overshoot_byte_nanoseconds
            .iter()
            .filter(|((period, _), _)| period_matches(*period))
            .map(|(_, amount)| *amount)
            .fold(0u128, u128::saturating_add);
        usage.ephemeral_storage_byte_nanoseconds = usage
            .ephemeral_storage_byte_nanoseconds
            .saturating_add(ephemeral_storage_overshoot);
        usage
    }

    #[cfg(test)]
    pub(crate) fn fuel_delta(&self) -> i64 {
        self.delta.load(Ordering::Acquire)
    }

    pub fn borrow_fuel(&self, amount: u64) -> bool {
        matches!(
            self.borrow_fuel_with_revision(amount),
            FuelBorrow::Borrowed { .. }
        )
    }

    pub(crate) fn borrow_fuel_with_revision(&self, amount: u64) -> FuelBorrow {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        if self.metering.compute {
            self.update_usage_period_locked(&mut revision_state, AccountUsagePeriod::current());
        }
        let revision = revision_state.current_policy_revision;
        let period = revision_state.current_period;
        let generation = revision_state
            .monthly_policy
            .as_ref()
            .map_or(0, |gate| gate.settled_generation);
        if amount == 0 {
            return FuelBorrow::Borrowed {
                amount: 0,
                revision,
                generation,
                period,
            };
        }
        if !self.metering.compute {
            return FuelBorrow::Borrowed {
                amount,
                revision,
                generation,
                period,
            };
        }
        let Some(gate) = &revision_state.monthly_policy else {
            return FuelBorrow::Borrowed {
                amount,
                revision,
                generation,
                period,
            };
        };
        let borrowed = match gate.mode {
            MonthlyUsageMode::AllowOverage => amount,
            MonthlyUsageMode::HardLimit => {
                amount.min(self.effective_fuel_with_revision_state(&revision_state))
            }
        };

        if borrowed > 0 {
            let amt_i64 = borrowed.min(i64::MAX as u64) as i64;
            self.delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                    Some(d.saturating_add(amt_i64))
                })
                .ok();
            record_fuel_borrow(borrowed);
            FuelBorrow::Borrowed {
                amount: borrowed,
                revision,
                generation,
                period,
            }
        } else {
            FuelBorrow::Exhausted {
                revision,
                generation,
                period,
            }
        }
    }

    pub(crate) fn monthly_resource_capacity(
        &self,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceExhaustion> {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        let Some(gate) = revision_state.monthly_policy.as_ref() else {
            return Ok(());
        };
        if gate.mode == MonthlyUsageMode::AllowOverage {
            return Ok(());
        }
        if gate.available_fuel.is_some()
            && self.effective_fuel_with_revision_state(&revision_state) == 0
        {
            return Err(MonthlyResourceExhaustion::Compute);
        }
        self.monthly_memory_and_storage_capacity_with_revision_state(&revision_state, agent_mode)
    }

    pub(crate) fn monthly_memory_and_storage_capacity(
        &self,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceExhaustion> {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        let Some(gate) = revision_state.monthly_policy.as_ref() else {
            return Ok(());
        };
        if gate.mode == MonthlyUsageMode::AllowOverage {
            return Ok(());
        }
        self.monthly_memory_and_storage_capacity_with_revision_state(&revision_state, agent_mode)
    }

    fn monthly_memory_and_storage_capacity_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceExhaustion> {
        let gate = revision_state
            .monthly_policy
            .as_ref()
            .expect("monthly policy was checked before resource capacity");
        if !self.has_monthly_memory_capacity_with_revision_state(revision_state) {
            return Err(MonthlyResourceExhaustion::Memory);
        }
        let storage_enabled = match agent_mode {
            AgentMode::Durable => gate.available_durable_storage_byte_nanoseconds.is_some(),
            AgentMode::Ephemeral => gate.available_ephemeral_storage_byte_nanoseconds.is_some(),
        };
        if storage_enabled
            && self
                .effective_storage_byte_nanoseconds_with_revision_state(revision_state, agent_mode)
                == 0
        {
            return Err(match agent_mode {
                AgentMode::Durable => MonthlyResourceExhaustion::DurableStorage,
                AgentMode::Ephemeral => MonthlyResourceExhaustion::EphemeralStorage,
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn has_monthly_memory_capacity(&self) -> bool {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.has_monthly_memory_capacity_with_revision_state(&revision_state)
    }

    fn has_monthly_memory_capacity_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> bool {
        revision_state.monthly_policy.as_ref().is_none_or(|gate| {
            gate.mode == MonthlyUsageMode::AllowOverage
                || gate.available_memory_byte_nanoseconds.is_none()
                || self.effective_memory_byte_nanoseconds_with_revision_state(revision_state) > 0
        })
    }

    pub(crate) fn hard_limit_attribution_after(
        &self,
        generation: Option<u64>,
    ) -> Option<(u64, AccountUsagePeriod)> {
        let revision_state = self.usage_revision_state.lock().unwrap();
        revision_state.monthly_policy.as_ref().and_then(|gate| {
            (gate.mode == MonthlyUsageMode::HardLimit
                && gate.available_fuel.is_some()
                && self.effective_fuel_with_revision_state(&revision_state) == 0
                && generation.is_none_or(|generation| gate.settled_generation > generation))
            .then_some((revision_state.current_policy_revision, gate.period))
        })
    }

    pub(crate) fn record_hard_limit_overshoot(
        &self,
        amount: u64,
        revision: u64,
        period: AccountUsagePeriod,
    ) {
        if amount > 0 {
            self.record_fuel_delta_for_revision_and_period(
                revision,
                period,
                amount.min(i64::MAX as u64) as i64,
            );
        }
    }

    pub fn return_fuel(&self, amount: u64) {
        let revision = self
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision;
        self.return_fuel_for_revision(amount, revision);
    }

    pub(crate) fn return_fuel_for_revision(&self, amount: u64, revision: u64) {
        let period = self.usage_revision_state.lock().unwrap().current_period;
        self.return_fuel_for_revision_and_period(amount, revision, period);
    }

    pub(crate) fn return_fuel_for_revision_and_period(
        &self,
        amount: u64,
        revision: u64,
        period: AccountUsagePeriod,
    ) {
        if !self.metering.compute {
            return;
        }
        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.record_fuel_delta_for_revision_and_period(revision, period, -amt_i64);
        record_fuel_return(amount);
    }

    pub fn record_overdraft_debt(&self, amount: u64) {
        let revision = self
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision;
        self.record_overdraft_debt_for_revision(amount, revision);
    }

    pub(crate) fn record_overdraft_debt_for_revision(&self, amount: u64, revision: u64) {
        if !self.metering.compute || amount == 0 {
            return;
        }

        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.record_fuel_delta_for_revision(revision, amt_i64);
        record_ephemeral_overdraft_fuel(amount);
    }

    fn record_fuel_delta_for_revision(&self, revision: u64, fuel_delta: i64) {
        let period = self.usage_revision_state.lock().unwrap().current_period;
        self.record_fuel_delta_for_revision_and_period(revision, period, fuel_delta);
    }

    fn record_fuel_delta_for_revision_and_period(
        &self,
        revision: u64,
        period: AccountUsagePeriod,
        fuel_delta: i64,
    ) {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        assert!(
            revision <= revision_state.current_policy_revision,
            "fuel usage references future revision {revision}; current revision is {}",
            revision_state.current_policy_revision
        );
        if revision == revision_state.current_policy_revision
            && period == revision_state.current_period
        {
            self.delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(fuel_delta))
                })
                .ok();
        } else {
            let monthly_usage_mode_revision = revision_state.current_mode_revision;
            revision_state.pending.push_back(CapturedUsageUpdate {
                update: ResourceUsageUpdate {
                    period,
                    monthly_usage_mode_revision,
                    monthly_policy_revision: revision,
                    memory_byte_nanoseconds_remainder: 0,
                    durable_storage_byte_nanoseconds_remainder: 0,
                    ephemeral_storage_byte_nanoseconds_remainder: 0,
                    fuel_delta,
                    http_call_count_delta: 0,
                    rpc_call_count_delta: 0,
                    durable_storage_byte_seconds_delta: 0,
                    ephemeral_storage_byte_seconds_delta: 0,
                    memory_gb_seconds_delta: 0,
                    metering: golem_service_base::clients::registry::ResourceUsageMetering {
                        compute: self.metering.compute,
                        memory: self.metering.memory,
                        filesystem: self.metering.filesystem,
                    },
                },
                durable_memory_gb_seconds_delta: 0,
                ephemeral_memory_gb_seconds_delta: 0,
            });
        }
    }

    pub fn max_memory_limit(&self) -> usize {
        self.max_memory.load(Ordering::Acquire)
    }

    pub(crate) fn register_agent_memory_limit_target(
        &self,
        target: Weak<dyn AgentMemoryLimitTarget>,
    ) {
        self.agent_memory_limit_targets.lock().unwrap().push(target);
    }

    pub(crate) fn update_memory_limit(&self, limit: u64) {
        let previous = self.max_memory.swap(limit as usize, Ordering::AcqRel) as u64;
        if limit >= previous {
            return;
        }

        let targets = {
            let mut registered = self.agent_memory_limit_targets.lock().unwrap();
            let mut targets = Vec::with_capacity(registered.len());
            registered.retain(|target| {
                target.upgrade().is_some_and(|target| {
                    targets.push(target);
                    true
                })
            });
            targets
        };
        for target in targets {
            target.enforce_limit(limit);
        }
    }

    pub fn max_table_elements_limit(&self) -> usize {
        self.max_table_elements.load(Ordering::Acquire)
    }

    pub fn max_disk_space_limit(&self) -> u64 {
        self.max_disk_space.load(Ordering::Acquire)
    }

    pub(crate) fn register_agent_filesystem_limit_target(
        self: &Arc<Self>,
        owned_agent_id: OwnedAgentId,
        update: impl Fn(u64) -> AgentFilesystemLimitUpdate + Send + Sync + 'static,
    ) -> AgentFilesystemLimitRegistration {
        let target = Arc::new(AgentFilesystemLimitTarget {
            update: Arc::new(update),
        });
        self.agent_filesystem_limit_targets
            .upsert_sync(owned_agent_id.clone(), Arc::downgrade(&target));
        AgentFilesystemLimitRegistration {
            resource_limits: Arc::clone(self),
            owned_agent_id,
            target,
        }
    }

    #[doc(hidden)]
    pub async fn apply_agent_filesystem_limit(
        &self,
        allocated_bytes: u64,
    ) -> Result<(), (OwnedAgentId, WorkerExecutorError)> {
        let _update = self.filesystem_limit_update.lock().await;
        self.max_disk_space
            .store(allocated_bytes, Ordering::Release);
        let mut targets = Vec::new();
        self.agent_filesystem_limit_targets
            .iter_sync(|owned_agent_id, target| {
                if let Some(target) = target.upgrade() {
                    targets.push((owned_agent_id.clone(), target));
                }
                true
            });
        let mut first_error = None;
        for (owned_agent_id, target) in targets {
            if let Err(error) = (target.update)(allocated_bytes).await
                && first_error.is_none()
            {
                first_error = Some((owned_agent_id, error));
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub fn record_storage_byte_seconds(&self, mode: AgentMode, amount: i64) {
        if amount > 0 {
            self.record_storage_settlement(
                mode,
                ByteTimeSettlement {
                    units: amount as u128,
                    remainder: 0,
                },
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn record_resource_settlement(
        &self,
        mode: AgentMode,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) {
        let period = self.advance_to_current_usage_period();
        self.record_resource_settlement_for_period(mode, period, memory, storage);
    }

    pub(crate) fn record_resource_settlement_for_period(
        &self,
        mode: AgentMode,
        period: AccountUsagePeriod,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) {
        let Some(accumulator) = &self.account_usage_accumulator else {
            return;
        };
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        if period < revision_state.current_period {
            let captured = self.capture_historical_byte_time_settlements(
                revision_state.current_mode_revision,
                revision_state.current_policy_revision,
                period,
                mode,
                memory,
                storage,
            );
            revision_state.pending.extend(captured);
            return;
        }
        self.update_usage_period_locked(&mut revision_state, period);
        let memory_limit = self.memory_settlement_limit(&revision_state);
        let storage_limit = self.storage_settlement_limit(&revision_state, mode);
        let mut accumulator = accumulator.lock().unwrap();
        if self.metering.memory {
            let overshoot = accumulator.add_memory_settlement(mode, memory, memory_limit);
            Self::record_hard_limit_memory_overshoot(&mut revision_state, overshoot);
        }
        if self.metering.filesystem {
            Self::record_storage_settlement_locked(
                &mut revision_state,
                &mut accumulator,
                mode,
                storage,
                storage_limit,
            );
        }
    }

    fn capture_historical_byte_time_settlements(
        &self,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        period: AccountUsagePeriod,
        mode: AgentMode,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) -> Vec<CapturedUsageUpdate> {
        let mut accumulator = AccountUsageAccumulator::new(self.metering);
        accumulator.add_memory_settlement(mode, memory, None);
        accumulator.add_storage_settlement(mode, storage, None);
        let mut batches = Vec::new();
        while accumulator.is_active() {
            let captured = accumulator.capture(false);
            batches.push(self.captured_byte_time_update_with_policy_revision(
                monthly_usage_mode_revision,
                monthly_policy_revision,
                period,
                captured,
            ));
        }
        let remainder = accumulator.capture(true);
        if !remainder.is_zero() {
            batches.push(self.captured_byte_time_update_with_policy_revision(
                monthly_usage_mode_revision,
                monthly_policy_revision,
                period,
                remainder,
            ));
        }
        batches
    }

    fn captured_byte_time_update_with_policy_revision(
        &self,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        period: AccountUsagePeriod,
        captured: CapturedAccountUsage,
    ) -> CapturedUsageUpdate {
        CapturedUsageUpdate {
            update: ResourceUsageUpdate {
                period,
                monthly_usage_mode_revision,
                monthly_policy_revision,
                memory_byte_nanoseconds_remainder: captured.memory_byte_nanoseconds_remainder,
                durable_storage_byte_nanoseconds_remainder: captured
                    .durable_storage_byte_nanoseconds_remainder,
                ephemeral_storage_byte_nanoseconds_remainder: captured
                    .ephemeral_storage_byte_nanoseconds_remainder,
                fuel_delta: 0,
                memory_gb_seconds_delta: captured.memory_gb_seconds,
                http_call_count_delta: 0,
                rpc_call_count_delta: 0,
                durable_storage_byte_seconds_delta: captured.durable_storage_byte_seconds,
                ephemeral_storage_byte_seconds_delta: captured.ephemeral_storage_byte_seconds,
                metering: golem_service_base::clients::registry::ResourceUsageMetering {
                    compute: self.metering.compute,
                    memory: self.metering.memory,
                    filesystem: self.metering.filesystem,
                },
            },
            durable_memory_gb_seconds_delta: captured.durable_memory_gb_seconds,
            ephemeral_memory_gb_seconds_delta: captured.ephemeral_memory_gb_seconds,
        }
    }

    #[cfg(test)]
    fn record_resource_usage(
        &self,
        mode: AgentMode,
        memory_gb_seconds: i64,
        storage_byte_seconds: i64,
    ) {
        self.record_resource_settlement(
            mode,
            ByteTimeSettlement {
                units: memory_gb_seconds.max(0) as u128,
                remainder: 0,
            },
            ByteTimeSettlement {
                units: storage_byte_seconds.max(0) as u128,
                remainder: 0,
            },
        );
    }

    pub(crate) fn register_resource_usage_flusher(&self, flusher: Weak<dyn ResourceUsageFlusher>) {
        self.resource_usage_flushers.lock().unwrap().push(flusher);
    }

    fn flush_active_resource_usage(&self) {
        let flushers = {
            let mut registered = self.resource_usage_flushers.lock().unwrap();
            let mut flushers = Vec::with_capacity(registered.len());
            registered.retain(|flusher| match flusher.upgrade() {
                Some(flusher) => {
                    flushers.push(flusher);
                    true
                }
                None => false,
            });
            flushers
        };
        for flusher in flushers {
            flusher.flush_usage();
        }
    }

    #[cfg(test)]
    pub(crate) fn record_storage_remainder(&self, mode: AgentMode, remainder: u128) {
        if !self.metering.filesystem || remainder == 0 {
            return;
        }
        let _revision_state = self.usage_revision_state.lock().unwrap();
        self.account_usage_accumulator
            .as_ref()
            .expect("filesystem metering is enabled")
            .lock()
            .unwrap()
            .add_storage_settlement(
                mode,
                ByteTimeSettlement {
                    units: 0,
                    remainder,
                },
                None,
            );
    }

    /// Returns the local durable storage delta for production-context executor tests.
    #[doc(hidden)]
    pub fn flush_durable_storage_byte_seconds_for_test(&self) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .storage(AgentMode::Durable)
                    .min(i64::MAX as u128) as i64
            })
    }

    fn capture_usage_update(&self, refresh_threshold_secs: i64) -> Option<CapturedUsageUpdate> {
        self.capture_usage_update_at(AccountUsagePeriod::current(), refresh_threshold_secs)
    }

    fn capture_usage_update_at(
        &self,
        period: AccountUsagePeriod,
        refresh_threshold_secs: i64,
    ) -> Option<CapturedUsageUpdate> {
        self.flush_active_resource_usage();
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut revision_state, period);
        if let Some(captured) = revision_state.pending.pop_front() {
            self.mark_in_flight(&captured, &mut revision_state);
            return Some(captured);
        }

        let active = (self.metering.compute && self.delta.load(Ordering::Acquire) != 0)
            || self
                .account_usage_accumulator
                .as_ref()
                .is_some_and(|accumulator| accumulator.lock().unwrap().is_active())
            || self.unsynced_http_calls.load(Ordering::Acquire) > 0
            || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0;
        let stale = self.secs_since_last_refresh() >= refresh_threshold_secs;

        if !active && !stale {
            return None;
        }

        let captured = self.capture_current_usage(
            revision_state.current_mode_revision,
            revision_state.current_policy_revision,
            revision_state.current_period,
            false,
        );
        self.mark_in_flight(&captured, &mut revision_state);
        Some(captured)
    }

    fn capture_current_usage(
        &self,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        period: AccountUsagePeriod,
        include_remainders: bool,
    ) -> CapturedUsageUpdate {
        let fuel_delta = if self.metering.compute {
            self.delta.swap(0, Ordering::AcqRel)
        } else {
            0
        };
        let captured_usage = self
            .account_usage_accumulator
            .as_ref()
            .map_or_else(CapturedAccountUsage::default, |accumulator| {
                accumulator.lock().unwrap().capture(include_remainders)
            });
        let mut captured = self.captured_byte_time_update_with_policy_revision(
            monthly_usage_mode_revision,
            monthly_policy_revision,
            period,
            captured_usage,
        );
        captured.update.fuel_delta = fuel_delta;
        captured.update.http_call_count_delta = self.unsynced_http_calls.swap(0, Ordering::AcqRel);
        captured.update.rpc_call_count_delta = self.unsynced_rpc_calls.swap(0, Ordering::AcqRel);
        captured
    }

    fn mark_in_flight(
        &self,
        captured: &CapturedUsageUpdate,
        revision_state: &mut UsageRevisionState,
    ) {
        if captured.update.http_call_count_delta > 0 {
            self.syncing_http_calls
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(captured.update.http_call_count_delta))
                })
                .ok();
        }
        if captured.update.rpc_call_count_delta > 0 {
            self.syncing_rpc_calls
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(captured.update.rpc_call_count_delta))
                })
                .ok();
        }
        if captured.update.fuel_delta != 0 {
            self.in_flight_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.update.fuel_delta))
                })
                .ok();
        }
        if captured.update.memory_gb_seconds_delta != 0 {
            self.in_flight_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.update.memory_gb_seconds_delta))
                })
                .ok();
        }
        if captured.durable_memory_gb_seconds_delta != 0 {
            self.in_flight_durable_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.durable_memory_gb_seconds_delta))
                })
                .ok();
        }
        if captured.ephemeral_memory_gb_seconds_delta != 0 {
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.ephemeral_memory_gb_seconds_delta))
                })
                .ok();
        }
        let usage = LocalMonthlyUsage::from_update(&captured.update);
        if !usage.is_zero()
            && let Some(gate) = &mut revision_state.monthly_policy
        {
            gate.unassigned_in_flight_usage
                .entry(captured.update.period)
                .or_default()
                .saturating_add_assign(usage);
        }
    }

    #[cfg(test)]
    fn update_policy_revision(&self, new_revision: u64) {
        self.flush_active_resource_usage();
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_revision_locked(&mut revision_state, new_revision);
    }

    fn update_usage_revision_locked(
        &self,
        revision_state: &mut UsageRevisionState,
        new_revision: u64,
    ) {
        if revision_state.current_policy_revision >= new_revision {
            return;
        }

        loop {
            let active = (self.metering.compute && self.delta.load(Ordering::Acquire) != 0)
                || self
                    .account_usage_accumulator
                    .as_ref()
                    .is_some_and(|accumulator| accumulator.lock().unwrap().is_active())
                || self.unsynced_http_calls.load(Ordering::Acquire) > 0
                || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0;
            if !active {
                break;
            }
            let captured = self.capture_current_usage(
                revision_state.current_mode_revision,
                revision_state.current_policy_revision,
                revision_state.current_period,
                false,
            );
            revision_state.pending.push_back(captured);
        }
        let captured = self.capture_current_usage(
            revision_state.current_mode_revision,
            revision_state.current_policy_revision,
            revision_state.current_period,
            true,
        );
        if captured.update.memory_byte_nanoseconds_remainder != 0
            || captured.update.durable_storage_byte_nanoseconds_remainder != 0
            || captured.update.ephemeral_storage_byte_nanoseconds_remainder != 0
        {
            revision_state.pending.push_back(captured);
        }
        revision_state.current_policy_revision = new_revision;
    }

    fn update_usage_period_locked(
        &self,
        revision_state: &mut UsageRevisionState,
        new_period: AccountUsagePeriod,
    ) {
        if revision_state.current_period >= new_period {
            return;
        }

        loop {
            let active = (self.metering.compute && self.delta.load(Ordering::Acquire) != 0)
                || self
                    .account_usage_accumulator
                    .as_ref()
                    .is_some_and(|accumulator| accumulator.lock().unwrap().is_active())
                || self.unsynced_http_calls.load(Ordering::Acquire) > 0
                || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0;
            if !active {
                break;
            }
            let captured = self.capture_current_usage(
                revision_state.current_mode_revision,
                revision_state.current_policy_revision,
                revision_state.current_period,
                false,
            );
            revision_state.pending.push_back(captured);
        }
        let captured = self.capture_current_usage(
            revision_state.current_mode_revision,
            revision_state.current_policy_revision,
            revision_state.current_period,
            true,
        );
        if captured.update.memory_byte_nanoseconds_remainder != 0
            || captured.update.durable_storage_byte_nanoseconds_remainder != 0
            || captured.update.ephemeral_storage_byte_nanoseconds_remainder != 0
        {
            revision_state.pending.push_back(captured);
        }
        revision_state.current_period = new_period;
    }

    fn begin_monthly_refresh(
        &self,
        generation: u64,
        update: &ResourceUsageUpdate,
        durable_memory_gb_seconds: i64,
        ephemeral_memory_gb_seconds: i64,
    ) {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        if let Some(gate) = &mut revision_state.monthly_policy {
            gate.refresh_generation = gate.refresh_generation.max(generation);
            let usage = LocalMonthlyUsage::from_update(update);
            if !usage.is_zero() {
                let remaining_unassigned = gate
                    .unassigned_in_flight_usage
                    .get(&update.period)
                    .copied()
                    .unwrap_or_default();
                let mut remaining_unassigned = remaining_unassigned;
                remaining_unassigned.saturating_sub_assign(usage);
                if remaining_unassigned.is_zero() {
                    gate.unassigned_in_flight_usage.remove(&update.period);
                } else {
                    gate.unassigned_in_flight_usage
                        .insert(update.period, remaining_unassigned);
                }
                let previous = gate.in_flight_usage.insert(
                    generation,
                    InFlightMonthlyUsage {
                        captured: CapturedUsageUpdate {
                            update: *update,
                            durable_memory_gb_seconds_delta: durable_memory_gb_seconds,
                            ephemeral_memory_gb_seconds_delta: ephemeral_memory_gb_seconds,
                        },
                        usage,
                    },
                );
                assert!(previous.is_none(), "refresh generation must be unique");
            }
        }
    }

    fn finish_monthly_delivery(
        &self,
        generation: u64,
        gate: &mut MonthlyPolicyGate,
    ) -> Option<InFlightMonthlyUsage> {
        let delivered = gate.in_flight_usage.remove(&generation);
        if let Some(delivered) = delivered {
            self.in_flight_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(in_flight.saturating_sub(delivered.usage.fuel as i64))
                })
                .ok();
            self.in_flight_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight.saturating_sub(delivered.captured.update.memory_gb_seconds_delta),
                    )
                })
                .ok();
            self.in_flight_durable_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight
                            .saturating_sub(delivered.captured.durable_memory_gb_seconds_delta),
                    )
                })
                .ok();
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight
                            .saturating_sub(delivered.captured.ephemeral_memory_gb_seconds_delta),
                    )
                })
                .ok();
        }
        delivered
    }

    #[cfg(test)]
    fn apply_monthly_snapshot(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
        usage_update_applied: bool,
    ) -> bool {
        self.apply_monthly_snapshot_with_policy_revision(
            generation,
            monthly_policy,
            monthly_usage_mode_revision,
            monthly_usage_mode_revision,
            usage_update_applied,
        )
    }

    fn apply_monthly_snapshot_with_policy_revision(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        usage_update_applied: bool,
    ) -> bool {
        self.flush_active_resource_usage();
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        let current_period = revision_state.current_period;
        let current_policy_revision = revision_state.current_policy_revision;
        if revision_state.monthly_policy.is_none() {
            self.update_usage_revision_locked(&mut revision_state, monthly_policy_revision);
            revision_state.current_mode_revision = monthly_usage_mode_revision;
            self.update_usage_period_locked(&mut revision_state, monthly_policy.period);
            return true;
        }
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("monthly policy gate was checked above");
        let delivered = self.finish_monthly_delivery(generation, gate);
        if generation != gate.refresh_generation || generation <= gate.settled_generation {
            if let Some(captured) =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied)
            {
                revision_state.pending.push_front(captured);
            }
            return false;
        }
        if monthly_policy_revision < current_policy_revision {
            let retry =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied);
            gate.settled_generation = generation;
            if let Some(captured) = retry {
                revision_state.pending.push_front(captured);
            }
            return false;
        }

        self.update_usage_revision_locked(&mut revision_state, monthly_policy_revision);
        revision_state.current_mode_revision = monthly_usage_mode_revision;
        self.update_usage_period_locked(&mut revision_state, monthly_policy.period);
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("monthly policy gate was checked above");
        if usage_update_applied
            && let Some(delivered) = delivered
            && delivered.captured.update.period > monthly_policy.period
        {
            let usage = delivered.usage.positive();
            if !usage.is_zero() {
                gate.stale_delivered_usage.push(StaleDeliveredUsage {
                    period: delivered.captured.update.period,
                    usage,
                    observed_refresh_generation: generation,
                });
            }
        }
        gate.failed_delivery_usage
            .retain(|period, _| *period >= monthly_policy.period);
        gate.hard_limit_memory_overshoot_byte_nanoseconds
            .retain(|(period, _), _| *period >= monthly_policy.period);
        gate.hard_limit_durable_storage_overshoot_byte_nanoseconds
            .retain(|(period, _), _| *period >= monthly_policy.period);
        gate.hard_limit_ephemeral_storage_overshoot_byte_nanoseconds
            .retain(|(period, _), _| *period >= monthly_policy.period);
        gate.stale_delivered_usage.retain(|retained| {
            retained.period > monthly_policy.period
                || (retained.period == monthly_policy.period
                    && generation <= retained.observed_refresh_generation)
        });
        let retry = if usage_update_applied {
            None
        } else {
            retain_unaccepted_delivery(gate, delivered, current_period, false)
        };
        gate.period = monthly_policy.period;
        gate.mode = monthly_policy.mode;
        gate.available_fuel = self
            .metering
            .compute
            .then_some(monthly_policy.available_fuel);
        gate.available_memory_byte_nanoseconds = self
            .metering
            .memory
            .then(|| monthly_policy_available_memory_byte_nanoseconds(&monthly_policy));
        gate.available_durable_storage_byte_nanoseconds = self.metering.filesystem.then(|| {
            monthly_policy_available_storage_byte_nanoseconds(&monthly_policy, AgentMode::Durable)
        });
        gate.available_ephemeral_storage_byte_nanoseconds = self.metering.filesystem.then(|| {
            monthly_policy_available_storage_byte_nanoseconds(&monthly_policy, AgentMode::Ephemeral)
        });
        gate.settled_generation = generation;
        if let Some(captured) = retry {
            revision_state.pending.push_front(captured);
        }
        true
    }

    fn finish_ambiguous_transport_failure(&self, generation: u64) -> bool {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        let current_period = revision_state.current_period;
        let Some(gate) = &mut revision_state.monthly_policy else {
            return true;
        };
        let delivered = self.finish_monthly_delivery(generation, gate);
        if let Some(delivered) = delivered
            && (delivered.captured.update.period == gate.period
                || delivered.captured.update.period == current_period)
        {
            gate.failed_delivery_usage
                .entry(delivered.captured.update.period)
                .or_default()
                .saturating_add_nonnegative_assign(delivered.usage);
        }
        if generation != gate.refresh_generation || generation <= gate.settled_generation {
            return false;
        }
        gate.settled_generation = generation;
        true
    }

    fn finish_confirmed_non_application(&self, generation: u64) -> bool {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        let current_period = revision_state.current_period;
        let Some(gate) = &mut revision_state.monthly_policy else {
            return true;
        };
        let delivered = self.finish_monthly_delivery(generation, gate);
        let retry = retain_unaccepted_delivery(gate, delivered, current_period, false);
        let latest = generation == gate.refresh_generation && generation > gate.settled_generation;
        if latest {
            gate.settled_generation = generation;
        }
        if let Some(captured) = retry {
            revision_state.pending.push_front(captured);
        }
        latest
    }

    #[cfg(test)]
    pub(crate) fn update_policy_revision_for_test(&self, new_revision: u64) {
        self.update_policy_revision(new_revision);
    }

    #[cfg(test)]
    pub(crate) fn apply_monthly_snapshot_for_test(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
    ) -> bool {
        self.begin_monthly_refresh_for_test(generation, 0);
        self.apply_monthly_snapshot(
            generation,
            monthly_policy,
            monthly_usage_mode_revision,
            true,
        )
    }

    #[cfg(test)]
    fn begin_monthly_refresh_for_test(&self, generation: u64, fuel_delta: i64) {
        self.begin_monthly_refresh_with_memory_for_test(generation, fuel_delta, 0);
    }

    #[cfg(test)]
    fn begin_monthly_refresh_with_memory_for_test(
        &self,
        generation: u64,
        fuel_delta: i64,
        memory_gb_seconds_delta: i64,
    ) {
        let period = self.usage_revision_state.lock().unwrap().current_period;
        let update = ResourceUsageUpdate {
            period,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta,
            metering: golem_service_base::clients::registry::ResourceUsageMetering {
                compute: self.metering.compute,
                memory: self.metering.memory,
                filesystem: self.metering.filesystem,
            },
        };
        self.begin_monthly_refresh(generation, &update, memory_gb_seconds_delta, 0);
    }

    #[cfg(test)]
    pub(crate) fn capture_usage_update_for_test(&self) -> ResourceUsageUpdate {
        self.capture_usage_update(i64::MAX)
            .expect("expected a pending resource usage update")
            .update
    }

    #[cfg(test)]
    pub(crate) fn capture_usage_update_at_for_test(
        &self,
        period: AccountUsagePeriod,
    ) -> ResourceUsageUpdate {
        self.capture_usage_update_at(period, i64::MAX)
            .expect("expected a pending resource usage update")
            .update
    }

    #[cfg(test)]
    pub(crate) fn capture_byte_time_usage_for_test(&self) -> (i64, i64) {
        let captured = self
            .capture_usage_update(i64::MAX)
            .expect("active byte-time usage was not captured");
        (
            captured.update.memory_gb_seconds_delta,
            captured.update.durable_storage_byte_seconds_delta,
        )
    }

    pub fn record_memory_gb_seconds(&self, mode: AgentMode, amount: i64) {
        if amount > 0 {
            self.record_memory_settlement(
                mode,
                ByteTimeSettlement {
                    units: amount as u128,
                    remainder: 0,
                },
            );
        }
    }

    pub(crate) fn record_memory_settlement(&self, mode: AgentMode, settlement: ByteTimeSettlement) {
        let period = self.advance_to_current_usage_period();
        self.record_memory_settlement_for_period(mode, period, settlement);
    }

    pub(crate) fn record_memory_settlement_for_period(
        &self,
        mode: AgentMode,
        period: AccountUsagePeriod,
        settlement: ByteTimeSettlement,
    ) {
        if self.metering.memory {
            self.record_resource_settlement_for_period(
                mode,
                period,
                settlement,
                ByteTimeSettlement::default(),
            );
        }
    }

    fn record_storage_settlement(&self, mode: AgentMode, settlement: ByteTimeSettlement) {
        let period = self.advance_to_current_usage_period();
        self.record_storage_settlement_for_period(mode, period, settlement);
    }

    fn advance_to_current_usage_period(&self) -> AccountUsagePeriod {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut revision_state, AccountUsagePeriod::current());
        revision_state.current_period
    }

    fn record_storage_settlement_for_period(
        &self,
        mode: AgentMode,
        period: AccountUsagePeriod,
        settlement: ByteTimeSettlement,
    ) {
        if self.metering.filesystem {
            self.record_resource_settlement_for_period(
                mode,
                period,
                ByteTimeSettlement::default(),
                settlement,
            );
        }
    }

    fn record_storage_settlement_locked(
        revision_state: &mut UsageRevisionState,
        accumulator: &mut AccountUsageAccumulator,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
        maximum_byte_nanoseconds: Option<u128>,
    ) {
        let overshoot =
            accumulator.add_storage_settlement(mode, settlement, maximum_byte_nanoseconds);
        Self::record_hard_limit_storage_overshoot(revision_state, mode, overshoot);
    }

    #[cfg(test)]
    pub(crate) fn memory_gb_seconds_delta(&self, mode: AgentMode) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .memory(mode)
                    .min(i64::MAX as u128) as i64
            })
    }

    #[cfg(test)]
    pub(crate) fn durable_byte_seconds_delta(&self) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .storage(AgentMode::Durable)
                    .min(i64::MAX as u128) as i64
            })
    }

    #[cfg(test)]
    pub(crate) fn ephemeral_byte_seconds_delta(&self) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .storage(AgentMode::Ephemeral)
                    .min(i64::MAX as u128) as i64
            })
    }

    /// Returns the number of HTTP calls remaining in this billing period from the
    /// local perspective: the server's last-known available count minus calls that
    /// have been made but not yet synced (unsynced) or are currently being synced
    /// (syncing).
    pub fn remaining_http_calls(&self) -> u64 {
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.remaining_http_calls_with_revision_state(&revision_state)
    }

    fn remaining_http_calls_with_revision_state(&self, revision_state: &UsageRevisionState) -> u64 {
        let available = self
            .available_http_calls_from_server
            .load(Ordering::Acquire);
        let unsynced = self.unsynced_http_calls.load(Ordering::Acquire);
        let syncing = self.syncing_http_calls.load(Ordering::Acquire);
        let pending = revision_state
            .pending
            .iter()
            .map(|captured| captured.update.http_call_count_delta)
            .fold(0, u64::saturating_add);
        available
            .saturating_sub(unsynced)
            .saturating_sub(syncing)
            .saturating_sub(pending)
    }

    /// Returns the number of RPC calls remaining in this billing period.
    pub fn remaining_rpc_calls(&self) -> u64 {
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.remaining_rpc_calls_with_revision_state(&revision_state)
    }

    fn remaining_rpc_calls_with_revision_state(&self, revision_state: &UsageRevisionState) -> u64 {
        let available = self.available_rpc_calls_from_server.load(Ordering::Acquire);
        let unsynced = self.unsynced_rpc_calls.load(Ordering::Acquire);
        let syncing = self.syncing_rpc_calls.load(Ordering::Acquire);
        let pending = revision_state
            .pending
            .iter()
            .map(|captured| captured.update.rpc_call_count_delta)
            .fold(0, u64::saturating_add);
        available
            .saturating_sub(unsynced)
            .saturating_sub(syncing)
            .saturating_sub(pending)
    }

    /// Records one outgoing HTTP call against the monthly account quota.
    ///
    /// Returns `false` when the remaining HTTP call budget is zero,
    /// signalling that the worker should be suspended at the next opportunity.
    pub fn record_http_call(&self) -> bool {
        let revision_state = self.usage_revision_state.lock().unwrap();
        if self.remaining_http_calls_with_revision_state(&revision_state) == 0 {
            return false;
        }
        self.unsynced_http_calls
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |uhc| {
                Some(uhc.saturating_add(1))
            })
            .ok();
        true
    }

    /// Records one outgoing RPC call against the monthly account quota.
    ///
    /// Returns `false` when the remaining RPC call budget is zero.
    pub fn record_rpc_call(&self) -> bool {
        let revision_state = self.usage_revision_state.lock().unwrap();
        if self.remaining_rpc_calls_with_revision_state(&revision_state) == 0 {
            return false;
        }
        self.unsynced_rpc_calls
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |urc| {
                Some(urc.saturating_add(1))
            })
            .ok();
        true
    }

    pub fn max_concurrent_agents_per_executor(&self) -> u64 {
        self.max_concurrent_agents_per_executor
            .load(Ordering::Acquire)
    }

    /// Overwrite the concurrent agent limit. Used in tests to simulate a plan
    /// upgrade without going through the full registry/batch sync path.
    #[cfg(test)]
    pub(crate) fn set_max_concurrent_agents_per_executor(&self, limit: u64) {
        self.max_concurrent_agents_per_executor
            .store(limit, Ordering::Release);
    }
}

#[async_trait]
pub trait ResourceLimits: Send + Sync {
    // Get a handle to the shared resource limits entry for the account. This might be updated in the
    // background as fuel, HTTP call, and RPC call usage is reported to the registry service.
    async fn initialize_account(
        &self,
        account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError>;
}

pub fn configured(
    config: &ResourceLimitsConfig,
    metering: ResourceUsageMeteringConfig,
    registry_service: Arc<dyn RegistryService>,
    shutdown_token: CancellationToken,
) -> Arc<dyn ResourceLimits> {
    match config {
        ResourceLimitsConfig::Grpc(config) => ResourceLimitsGrpc::new(
            registry_service,
            config.batch_update_interval,
            config.limit_refresh_interval,
            metering,
            shutdown_token,
        ),
        ResourceLimitsConfig::Disabled(_) => {
            Arc::new(ConfiguredResourceLimitsDisabled { metering })
        }
    }
}

// Note:
// this is biased towards allowing borrows when it doubt, but might allow slight overborrowing temporarily.
// Internally we store deltas as i64 for simplicitly. If more fuel is consumed / returned within one update time slice
// than the i64 limits, those updates will be lost.
pub struct ResourceLimitsGrpc {
    client: Arc<dyn RegistryService>,
    entries: scc::HashMap<AccountId, Arc<OnceCell<Arc<AtomicResourceEntry>>>>,
    metering: ResourceUsageMeteringConfig,
    refresh_generation: AtomicU64,
}

impl ResourceLimitsGrpc {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        batch_update_interval: Duration,
        limit_refresh_interval: Duration,
        metering: ResourceUsageMeteringConfig,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let svc = Self {
            client: registry_service,
            entries: scc::HashMap::new(),
            metering,
            refresh_generation: AtomicU64::new(1),
        };
        let svc = Arc::new(svc);
        let svc_weak = Arc::downgrade(&svc);

        // Background task for batch updates
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(batch_update_interval);
            let refresh_threshold_secs = limit_refresh_interval.as_secs() as i64;
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        break;
                    }
                    _ = tick.tick() => {}
                }

                let svc_arc = match svc_weak.upgrade() {
                    Some(s) => s,
                    None => {
                        // service itself was dropped, we can exit
                        break;
                    }
                };

                svc_arc.send_batch(refresh_threshold_secs).await;
            }
        });

        svc
    }

    async fn fetch_resource_limits(
        &self,
        account_id: AccountId,
    ) -> Result<golem_service_base::model::ResourceLimits, WorkerExecutorError> {
        debug!("Fetching resource limits for account {account_id}");

        let last_known_limits = self
            .client
            .get_resource_limits(account_id)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed fetching resource limits: {}",
                    e.to_safe_string()
                ))
            })?;

        Ok(last_known_limits)
    }

    fn next_refresh_generation(&self) -> u64 {
        self.refresh_generation.fetch_add(1, Ordering::AcqRel)
    }

    async fn begin_monthly_refresh(
        &self,
        account_id: AccountId,
        generation: u64,
        update: &ResourceUsageUpdate,
        memory_by_mode: (i64, i64),
    ) {
        if let Some(cell) = self
            .entries
            .read_async(&account_id, |_, entry| entry.clone())
            .await
            && let Some(entry) = cell.get()
        {
            entry.begin_monthly_refresh(generation, update, memory_by_mode.0, memory_by_mode.1);
        }
    }

    /// Builds and sends a single batch to the registry covering:
    /// - active accounts with non-zero fuel, memory, storage, HTTP, or RPC deltas
    /// - otherwise-idle accounts past the refresh threshold
    ///
    /// On success, updates all entries via `update_last_known_limits`. On
    /// failure, drops the captured batch under the accepted bounded-loss semantics and
    /// resets in-flight quota tracking; stale idle accounts are retried next tick.
    async fn send_batch(&self, refresh_threshold_secs: i64) {
        async {
            let mut entries = Vec::new();
            self.entries
                .iter_async(|account_id, cell| {
                    if let Some(entry) = cell.get() {
                        entries.push((*account_id, entry.clone()));
                    }
                    true
                })
                .await;
            let mut updates = HashMap::new();
            let mut memory_mode_updates = HashMap::new();
            for (account_id, entry) in entries {
                if let Some(captured) = entry.capture_usage_update(refresh_threshold_secs) {
                    updates.insert(account_id, captured.update);
                    memory_mode_updates.insert(
                        account_id,
                        (
                            captured.durable_memory_gb_seconds_delta,
                            captured.ephemeral_memory_gb_seconds_delta,
                        ),
                    );
                }
            }

            if updates.is_empty() {
                return;
            }

            let mut pending_updates = updates.into_iter();
            loop {
                let updates: HashMap<_, _> = pending_updates.by_ref().take(256).collect();
                if updates.is_empty() {
                    break;
                }
                let refresh_generation = self.next_refresh_generation();
                for (account_id, update) in &updates {
                    self.begin_monthly_refresh(
                        *account_id,
                        refresh_generation,
                        update,
                        memory_mode_updates
                            .get(account_id)
                            .copied()
                            .unwrap_or_default(),
                    )
                    .await;
                }

                tracing::debug!(
                    "Sending batch: {} fuel, {} memory, {} durable storage, {} ephemeral storage, {} http, {} rpc, {} stale idle account(s)",
                    updates.values().filter(|u| u.fuel_delta != 0).count(),
                    updates
                        .values()
                        .filter(|u| u.memory_gb_seconds_delta != 0)
                        .count(),
                    updates
                        .values()
                        .filter(|u| u.durable_storage_byte_seconds_delta != 0)
                        .count(),
                    updates
                        .values()
                        .filter(|u| u.ephemeral_storage_byte_seconds_delta != 0)
                        .count(),
                    updates
                        .values()
                        .filter(|u| u.http_call_count_delta > 0)
                        .count(),
                    updates
                        .values()
                        .filter(|u| u.rpc_call_count_delta > 0)
                        .count(),
                    updates
                        .values()
                        .filter(|u| {
                            u.fuel_delta == 0
                                && u.memory_gb_seconds_delta == 0
                                && u.durable_storage_byte_seconds_delta == 0
                                && u.ephemeral_storage_byte_seconds_delta == 0
                                && u.http_call_count_delta == 0
                                && u.rpc_call_count_delta == 0
                        })
                        .count(),
                );

                // Send resource usage batch. The response refreshes all account limits
                // (fuel, memory, disk, per-invocation caps, and monthly call budgets)
                // for every account in `updates`.
                match self
                    .client
                    .batch_update_resource_usage(updates.clone())
                    .await
                {
                    Ok(updated_limits) => {
                        for (account_id, update) in &updates {
                            let Some(resource_limits) = updated_limits.0.get(account_id) else {
                                record_resource_usage_batch_update_failure();
                                error!(
                                    "Registry did not apply resource usage update for account {account_id}; retaining retryable byte-time usage"
                                );
                                self.finish_in_flight_update(
                                    *account_id,
                                    refresh_generation,
                                    AtomicResourceEntry::finish_confirmed_non_application,
                                )
                                .await;
                                continue;
                            };
                            if !resource_limits.usage_update_applied {
                                continue;
                            }
                            let durable = update.durable_storage_byte_seconds_delta;
                            let ephemeral = update.ephemeral_storage_byte_seconds_delta;
                            let (durable_memory, ephemeral_memory) = memory_mode_updates
                                .get(account_id)
                                .copied()
                                .unwrap_or_default();
                            if durable == 0
                                && ephemeral == 0
                                && durable_memory == 0
                                && ephemeral_memory == 0
                            {
                                continue;
                            }

                            let account_id = account_id.to_string();
                            if durable > 0 {
                                record_storage_byte_seconds(
                                    &account_id,
                                    AgentMode::Durable,
                                    durable,
                                );
                            }
                            if ephemeral > 0 {
                                record_storage_byte_seconds(
                                    &account_id,
                                    AgentMode::Ephemeral,
                                    ephemeral,
                                );
                            }
                            if durable_memory > 0 {
                                record_memory_gb_seconds(
                                    &account_id,
                                    AgentMode::Durable,
                                    durable_memory,
                                );
                            }
                            if ephemeral_memory > 0 {
                                record_memory_gb_seconds(
                                    &account_id,
                                    AgentMode::Ephemeral,
                                    ephemeral_memory,
                                );
                            }
                        }
                        for (account_id, resource_limits) in updated_limits.0 {
                            self.update_last_known_limits(
                                account_id,
                                resource_limits,
                                refresh_generation,
                            )
                            .await;
                        }
                    }
                    Err(err) => {
                        record_resource_usage_batch_update_failure();
                        error!("Failed to send batched resource usage updates: {}", err);
                        for (account_id, update) in &updates {
                            if update.fuel_delta != 0
                                || update.memory_gb_seconds_delta != 0
                                || update.memory_byte_nanoseconds_remainder != 0
                                || update.durable_storage_byte_seconds_delta != 0
                                || update.durable_storage_byte_nanoseconds_remainder != 0
                                || update.ephemeral_storage_byte_seconds_delta != 0
                                || update.ephemeral_storage_byte_nanoseconds_remainder != 0
                                || update.http_call_count_delta > 0
                                || update.rpc_call_count_delta > 0
                            {
                                error!(
                                    "Lost resource usage updates for account {account_id}: fuel_delta={}, memory_gb_seconds_delta={}, memory_byte_nanoseconds_remainder={}, durable_storage_byte_seconds_delta={}, durable_storage_byte_nanoseconds_remainder={}, ephemeral_storage_byte_seconds_delta={}, ephemeral_storage_byte_nanoseconds_remainder={}, http_call_count_delta={}, rpc_call_count_delta={}",
                                    update.fuel_delta,
                                    update.memory_gb_seconds_delta,
                                    update.memory_byte_nanoseconds_remainder,
                                    update.durable_storage_byte_seconds_delta,
                                    update.durable_storage_byte_nanoseconds_remainder,
                                    update.ephemeral_storage_byte_seconds_delta,
                                    update.ephemeral_storage_byte_nanoseconds_remainder,
                                    update.http_call_count_delta,
                                    update.rpc_call_count_delta,
                                );
                                self.finish_in_flight_update(
                                    *account_id,
                                    refresh_generation,
                                    AtomicResourceEntry::finish_ambiguous_transport_failure,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
        .instrument(info_span!("resource_limits_batch_update"))
        .await
    }

    async fn update_last_known_limits(
        &self,
        account_id: AccountId,
        updated_limits: golem_service_base::model::ResourceLimits,
        refresh_generation: u64,
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if !entry.apply_monthly_snapshot_with_policy_revision(
                refresh_generation,
                updated_limits.monthly_policy.clone(),
                updated_limits.monthly_usage_mode_revision,
                updated_limits.monthly_policy_revision,
                updated_limits.usage_update_applied,
            ) {
                return;
            }
            entry.update_memory_limit(updated_limits.max_memory_per_worker);
            entry.max_table_elements.store(
                updated_limits.max_table_elements_per_worker as usize,
                Ordering::Release,
            );
            let filesystem_limit_updated = match entry
                .apply_agent_filesystem_limit(updated_limits.max_disk_space_per_worker)
                .await
            {
                Ok(()) => true,
                Err((owned_agent_id, error)) => {
                    error!(
                        account_id = %account_id,
                        agent_id = %owned_agent_id,
                        limit = updated_limits.max_disk_space_per_worker,
                        error = %error,
                        "Failed to apply managed agent filesystem limit"
                    );
                    false
                }
            };
            entry.per_invocation_http_call_limit.store(
                updated_limits.per_invocation_http_call_limit,
                Ordering::Release,
            );
            entry.per_invocation_rpc_call_limit.store(
                updated_limits.per_invocation_rpc_call_limit,
                Ordering::Release,
            );
            entry.syncing_http_calls.store(0, Ordering::Release);
            entry
                .available_http_calls_from_server
                .store(updated_limits.available_http_calls, Ordering::Release);
            entry.syncing_rpc_calls.store(0, Ordering::Release);
            entry
                .available_rpc_calls_from_server
                .store(updated_limits.available_rpc_calls, Ordering::Release);
            entry.max_concurrent_agents_per_executor.store(
                updated_limits.max_concurrent_agents_per_executor,
                Ordering::Release,
            );
            entry
                .oplog_writes_per_second
                .store(updated_limits.oplog_writes_per_second, Ordering::Release);
            if filesystem_limit_updated {
                entry
                    .last_refresh_secs
                    .store(Utc::now().timestamp(), Ordering::Release);
            }
        }
    }

    async fn finish_in_flight_update(
        &self,
        account_id: AccountId,
        refresh_generation: u64,
        finish: fn(&AtomicResourceEntry, u64) -> bool,
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if !finish(entry, refresh_generation) {
                return;
            }
            entry.syncing_http_calls.store(0, Ordering::Release);
            entry.syncing_rpc_calls.store(0, Ordering::Release);
        }
    }
}

#[async_trait]
impl ResourceLimits for ResourceLimitsGrpc {
    async fn initialize_account(
        &self,
        account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        let cell = self
            .entries
            .entry_async(account_id)
            .await
            .or_insert_with(|| Arc::new(OnceCell::new()));

        let entry = cell
            .get_or_try_init(|| async {
                let refresh_generation = self.next_refresh_generation();
                let fetched = self.fetch_resource_limits(account_id).await?;
                Ok::<Arc<AtomicResourceEntry>, WorkerExecutorError>(Arc::new(
                    AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                        fetched.monthly_policy,
                        fetched.max_memory_per_worker as usize,
                        fetched.max_table_elements_per_worker as usize,
                        fetched.max_disk_space_per_worker,
                        fetched.per_invocation_http_call_limit,
                        fetched.per_invocation_rpc_call_limit,
                        fetched.available_http_calls,
                        fetched.available_rpc_calls,
                        fetched.max_concurrent_agents_per_executor,
                        fetched.oplog_writes_per_second,
                        self.metering,
                        fetched.monthly_usage_mode_revision,
                        fetched.monthly_policy_revision,
                        refresh_generation,
                    ),
                ))
            })
            .await?;

        Ok(entry.clone())
    }
}

pub struct ResourceLimitsDisabled;

struct ConfiguredResourceLimitsDisabled {
    metering: ResourceUsageMeteringConfig,
}

#[async_trait]
impl ResourceLimits for ConfiguredResourceLimitsDisabled {
    async fn initialize_account(
        &self,
        _account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        Ok(Arc::new(
            AtomicResourceEntry::new_with_all_limits_and_metering(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                self.metering,
            ),
        ))
    }
}

#[async_trait]
impl ResourceLimits for ResourceLimitsDisabled {
    async fn initialize_account(
        &self,
        _account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        Ok(Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::golem_config::GolemConfig;
    use crate::worker::{MonthlyResourceAdmission, monthly_resource_admission};
    use golem_common::model::AgentId;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType, ResolvedAgentType};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::TokenSecret;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
    use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
    use golem_service_base::custom_api::CompiledRoutes;
    use golem_service_base::mcp::CompiledMcp;
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::model::component::Component;
    use golem_service_base::model::environment::EnvironmentState;
    use golem_service_base::model::{
        AccountResourceLimits, ResourceLimits as ServiceResourceLimits,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use test_r::test;
    use tokio::sync::Semaphore;
    use uuid::Uuid;

    test_r::enable!();

    fn metered_entry_with_revision(revision: u64) -> AtomicResourceEntry {
        AtomicResourceEntry::new_with_all_limits_metering_and_revision(
            10_000,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: true,
                memory: true,
                filesystem: true,
            },
            revision,
        )
    }

    fn monthly_policy(available_fuel: u64) -> MonthlyResourcePolicy {
        MonthlyResourcePolicy {
            period: AccountUsagePeriod::current(),
            mode: MonthlyUsageMode::HardLimit,
            available_fuel,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        }
    }

    fn usage_period(year: i32, month: u32) -> AccountUsagePeriod {
        AccountUsagePeriod { year, month }
    }

    fn memory_policy(
        period: AccountUsagePeriod,
        mode: MonthlyUsageMode,
        available_memory_gb_seconds: u64,
    ) -> MonthlyResourcePolicy {
        MonthlyResourcePolicy {
            period,
            mode,
            available_fuel: u64::MAX,
            available_memory_gb_seconds,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        }
    }

    fn storage_policy(
        period: AccountUsagePeriod,
        mode: MonthlyUsageMode,
        available_durable_storage_byte_seconds: u64,
        available_ephemeral_storage_byte_seconds: u64,
    ) -> MonthlyResourcePolicy {
        MonthlyResourcePolicy {
            period,
            mode,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        }
    }

    fn compute_entry(
        period: AccountUsagePeriod,
        mode: MonthlyUsageMode,
        available_fuel: u64,
    ) -> AtomicResourceEntry {
        AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
            MonthlyResourcePolicy {
                period,
                mode,
                available_fuel,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: true,
                memory: false,
                filesystem: false,
            },
            7,
            7,
            0,
        )
    }

    fn memory_entry(
        period: AccountUsagePeriod,
        mode: MonthlyUsageMode,
        available_memory_gb_seconds: u64,
    ) -> AtomicResourceEntry {
        AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
            memory_policy(period, mode, available_memory_gb_seconds),
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: false,
            },
            7,
            7,
            0,
        )
    }

    fn storage_entry(
        period: AccountUsagePeriod,
        mode: MonthlyUsageMode,
        available_durable_storage_byte_seconds: u64,
        available_ephemeral_storage_byte_seconds: u64,
    ) -> AtomicResourceEntry {
        AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
            storage_policy(
                period,
                mode,
                available_durable_storage_byte_seconds,
                available_ephemeral_storage_byte_seconds,
            ),
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: false,
                memory: false,
                filesystem: true,
            },
            7,
            7,
            0,
        )
    }

    #[derive(Debug)]
    struct MemoryUsageFlusher {
        entry: Weak<AtomicResourceEntry>,
        amount: Mutex<Option<i64>>,
    }

    impl ResourceUsageFlusher for MemoryUsageFlusher {
        fn flush_usage(&self) {
            let Some(amount) = self.amount.lock().unwrap().take() else {
                return;
            };
            if let Some(entry) = self.entry.upgrade() {
                entry.record_memory_gb_seconds(AgentMode::Durable, amount);
            }
        }
    }

    #[derive(Debug)]
    struct RecordingMemoryLimitTarget {
        enforced: AtomicU64,
    }

    impl AgentMemoryLimitTarget for RecordingMemoryLimitTarget {
        fn enforce_limit(&self, limit: u64) {
            self.enforced.store(limit, Ordering::Release);
        }
    }

    #[test]
    async fn filesystem_limit_update_is_delivered_to_registered_target() {
        let entry = Arc::new(AtomicResourceEntry::new(
            0,
            0,
            0,
            AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE,
            1,
        ));
        let owned_agent_id = OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "live-filesystem-limit".to_string(),
            },
        );
        let observed = Arc::new(Mutex::new(Vec::new()));
        let _registration = entry.register_agent_filesystem_limit_target(owned_agent_id, {
            let observed = Arc::clone(&observed);
            move |allocated_bytes| {
                let observed = Arc::clone(&observed);
                Box::pin(async move {
                    observed.lock().unwrap().push(allocated_bytes);
                    Ok(())
                })
            }
        });

        entry.apply_agent_filesystem_limit(4096).await.unwrap();

        assert_eq!(entry.max_disk_space_limit(), 4096);
        assert_eq!(*observed.lock().unwrap(), vec![4096]);
    }

    #[test]
    async fn dropped_filesystem_limit_registration_detaches_target() {
        let entry = Arc::new(AtomicResourceEntry::new(
            0,
            0,
            0,
            AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE,
            1,
        ));
        let owned_agent_id = OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "detached-filesystem-limit".to_string(),
            },
        );
        let observed = Arc::new(Mutex::new(Vec::new()));
        let registration = entry.register_agent_filesystem_limit_target(owned_agent_id, {
            let observed = Arc::clone(&observed);
            move |allocated_bytes| {
                let observed = Arc::clone(&observed);
                Box::pin(async move {
                    observed.lock().unwrap().push(allocated_bytes);
                    Ok(())
                })
            }
        });
        drop(registration);

        entry.apply_agent_filesystem_limit(2048).await.unwrap();

        assert_eq!(entry.max_disk_space_limit(), 2048);
        assert!(observed.lock().unwrap().is_empty());
    }

    #[test]
    fn account_usage_accumulator_emits_oversized_settlements_in_bounded_batches() {
        let mut accumulator = AccountUsageAccumulator::default();
        let oversized = i64::MAX as u128 + 7;
        accumulator.add_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: oversized,
                remainder: 0,
            },
            None,
        );
        accumulator.add_storage_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: oversized,
                remainder: 0,
            },
            None,
        );

        assert_eq!(
            accumulator.capture(false),
            CapturedAccountUsage {
                memory_gb_seconds: i64::MAX,
                durable_memory_gb_seconds: i64::MAX,
                ephemeral_memory_gb_seconds: 0,
                durable_storage_byte_seconds: i64::MAX,
                ephemeral_storage_byte_seconds: 0,
                ..Default::default()
            }
        );
        assert_eq!(
            accumulator.capture(false),
            CapturedAccountUsage {
                memory_gb_seconds: 7,
                durable_memory_gb_seconds: 7,
                ephemeral_memory_gb_seconds: 0,
                durable_storage_byte_seconds: 7,
                ephemeral_storage_byte_seconds: 0,
                ..Default::default()
            }
        );
        assert!(!accumulator.is_active());
    }

    // -------------------------------------------------------------------------
    // AtomicResourceEntry
    // -------------------------------------------------------------------------

    #[test]
    fn delayed_pre_opt_in_usage_keeps_its_accrual_revision() {
        let entry = metered_entry_with_revision(3);
        assert!(entry.borrow_fuel(100));
        entry.record_resource_usage(AgentMode::Durable, 10, 30);
        entry.record_resource_usage(AgentMode::Ephemeral, 5, 7);
        assert!(entry.record_http_call());
        assert!(entry.record_rpc_call());

        entry.update_policy_revision(4);
        assert_eq!(entry.effective_fuel(), 9_900);
        assert!(entry.borrow_fuel(200));
        entry.record_resource_usage(AgentMode::Durable, 20, 40);

        let before_opt_in = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(before_opt_in.update.monthly_policy_revision, 3);
        assert_eq!(before_opt_in.update.fuel_delta, 100);
        assert_eq!(before_opt_in.update.memory_gb_seconds_delta, 15);
        assert_eq!(before_opt_in.update.http_call_count_delta, 1);
        assert_eq!(before_opt_in.update.rpc_call_count_delta, 1);
        assert_eq!(before_opt_in.update.durable_storage_byte_seconds_delta, 30);
        assert_eq!(before_opt_in.update.ephemeral_storage_byte_seconds_delta, 7);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 100);
        assert_eq!(entry.syncing_http_calls.load(Ordering::Acquire), 1);
        assert_eq!(entry.syncing_rpc_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            entry
                .in_flight_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            15
        );
        assert_eq!(
            entry
                .in_flight_durable_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            10
        );
        assert_eq!(
            entry
                .in_flight_ephemeral_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            5
        );
        let after_opt_in = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(after_opt_in.update.monthly_policy_revision, 4);
        assert_eq!(after_opt_in.update.fuel_delta, 200);
        assert_eq!(after_opt_in.update.memory_gb_seconds_delta, 20);
        assert_eq!(after_opt_in.update.durable_storage_byte_seconds_delta, 40);
    }

    #[test]
    fn older_policy_revision_cannot_roll_back_usage_attribution() {
        let entry = metered_entry_with_revision(7);

        entry.update_policy_revision(3);

        assert_eq!(
            entry
                .usage_revision_state
                .lock()
                .unwrap()
                .current_policy_revision,
            7
        );
    }

    #[test]
    fn unaccepted_deliveries_are_retained_only_for_relevant_periods() {
        let gate_period = usage_period(2026, 4);
        let current_period = usage_period(2026, 5);
        let unrelated_period = usage_period(2026, 3);
        for (period, retained) in [
            (gate_period, true),
            (current_period, true),
            (unrelated_period, false),
        ] {
            let entry = storage_entry(gate_period, MonthlyUsageMode::HardLimit, 10, 10);
            let mut revision_state = entry.usage_revision_state.lock().unwrap();
            revision_state.current_period = current_period;
            let gate = revision_state.monthly_policy.as_mut().unwrap();
            let captured = entry.captured_byte_time_update_with_policy_revision(
                7,
                7,
                period,
                CapturedAccountUsage {
                    durable_storage_byte_seconds: 1,
                    ..CapturedAccountUsage::default()
                },
            );
            assert!(
                retain_unaccepted_delivery(
                    gate,
                    Some(InFlightMonthlyUsage {
                        captured,
                        usage: LocalMonthlyUsage::from_update(&captured.update),
                    }),
                    current_period,
                    true,
                )
                .is_none()
            );
            assert_eq!(gate.stale_delivered_usage.len(), usize::from(retained));
        }

        for period in [gate_period, current_period, unrelated_period] {
            let entry = storage_entry(gate_period, MonthlyUsageMode::HardLimit, 10, 10);
            let mut revision_state = entry.usage_revision_state.lock().unwrap();
            revision_state.current_period = current_period;
            let gate = revision_state.monthly_policy.as_mut().unwrap();
            let mut captured = entry.captured_byte_time_update_with_policy_revision(
                7,
                7,
                period,
                CapturedAccountUsage {
                    durable_storage_byte_seconds: 1,
                    durable_storage_byte_nanoseconds_remainder: 3,
                    ..CapturedAccountUsage::default()
                },
            );
            captured.update.fuel_delta = 5;
            let retry = retain_unaccepted_delivery(
                gate,
                Some(InFlightMonthlyUsage {
                    captured,
                    usage: LocalMonthlyUsage::from_update(&captured.update),
                }),
                current_period,
                false,
            );
            let retry = retry.unwrap();
            assert_eq!(retry.update.period, captured.update.period);
            assert_eq!(retry.update.durable_storage_byte_seconds_delta, 1);
            assert_eq!(retry.update.durable_storage_byte_nanoseconds_remainder, 3);
            assert_eq!(retry.update.fuel_delta, 0);
            assert_eq!(
                gate.failed_delivery_usage
                    .get(&period)
                    .map(|usage| usage.fuel),
                (period == gate_period || period == current_period).then_some(5)
            );
        }
    }

    #[test]
    fn period_tagged_settlements_preserve_old_usage_and_storage_classes() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let entry = storage_entry(january, MonthlyUsageMode::AllowOverage, 100, 100);

        entry.record_resource_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 2,
                remainder: 7,
            },
        );
        entry.record_resource_settlement_for_period(
            AgentMode::Ephemeral,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 3,
                remainder: 11,
            },
        );
        entry.record_resource_settlement_for_period(
            AgentMode::Durable,
            february,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 5,
                remainder: 17,
            },
        );

        let old = entry.capture_usage_update_at_for_test(february);
        assert_eq!(old.period, january);
        assert_eq!(old.durable_storage_byte_seconds_delta, 2);
        assert_eq!(old.ephemeral_storage_byte_seconds_delta, 3);
        assert_eq!(old.durable_storage_byte_nanoseconds_remainder, 0);
        assert_eq!(old.ephemeral_storage_byte_nanoseconds_remainder, 0);

        let old_remainders = entry.capture_usage_update_at_for_test(february);
        assert_eq!(old_remainders.period, january);
        assert_eq!(old_remainders.durable_storage_byte_seconds_delta, 0);
        assert_eq!(old_remainders.durable_storage_byte_nanoseconds_remainder, 7);
        assert_eq!(
            old_remainders.ephemeral_storage_byte_nanoseconds_remainder,
            11
        );

        let current = entry.capture_usage_update_at_for_test(february);
        assert_eq!(current.period, february);
        assert_eq!(current.durable_storage_byte_seconds_delta, 5);
        assert_eq!(current.durable_storage_byte_nanoseconds_remainder, 0);
        assert_eq!(current.ephemeral_storage_byte_seconds_delta, 0);

        let march = usage_period(2030, 3);
        let current_remainder = entry.capture_usage_update_at_for_test(march);
        assert_eq!(current_remainder.period, february);
        assert_eq!(current_remainder.durable_storage_byte_seconds_delta, 0);
        assert_eq!(
            current_remainder.durable_storage_byte_nanoseconds_remainder,
            17
        );

        let memory = memory_entry(january, MonthlyUsageMode::AllowOverage, 100);
        memory.record_memory_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement {
                units: 1,
                remainder: 5,
            },
        );
        memory.record_memory_settlement_for_period(
            AgentMode::Ephemeral,
            february,
            ByteTimeSettlement {
                units: 4,
                remainder: 13,
            },
        );
        let old_memory = memory.capture_usage_update_at_for_test(february);
        assert_eq!(old_memory.period, january);
        assert_eq!(old_memory.memory_gb_seconds_delta, 1);
        assert_eq!(old_memory.memory_byte_nanoseconds_remainder, 0);
        let old_memory_remainder = memory.capture_usage_update_at_for_test(february);
        assert_eq!(old_memory_remainder.period, january);
        assert_eq!(old_memory_remainder.memory_gb_seconds_delta, 0);
        assert_eq!(old_memory_remainder.memory_byte_nanoseconds_remainder, 5);
        let current_memory = memory.capture_usage_update_at_for_test(march);
        assert_eq!(current_memory.period, february);
        assert_eq!(current_memory.memory_gb_seconds_delta, 4);
        assert_eq!(current_memory.memory_byte_nanoseconds_remainder, 0);
        let current_memory_remainder = memory.capture_usage_update_at_for_test(march);
        assert_eq!(current_memory_remainder.period, february);
        assert_eq!(current_memory_remainder.memory_gb_seconds_delta, 0);
        assert_eq!(
            current_memory_remainder.memory_byte_nanoseconds_remainder,
            13
        );
    }

    #[test]
    fn late_prior_period_settlements_keep_their_period_and_bypass_current_limits() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let storage = storage_entry(february, MonthlyUsageMode::HardLimit, 0, 0);

        storage.record_resource_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 2,
                remainder: 7,
            },
        );
        storage.record_resource_settlement_for_period(
            AgentMode::Ephemeral,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 3,
                remainder: 11,
            },
        );

        let durable = storage.capture_usage_update_at_for_test(february);
        assert_eq!(durable.period, january);
        assert_eq!(durable.durable_storage_byte_seconds_delta, 2);
        assert_eq!(durable.durable_storage_byte_nanoseconds_remainder, 0);
        let durable_remainder = storage.capture_usage_update_at_for_test(february);
        assert_eq!(durable_remainder.period, january);
        assert_eq!(durable_remainder.durable_storage_byte_seconds_delta, 0);
        assert_eq!(
            durable_remainder.durable_storage_byte_nanoseconds_remainder,
            7
        );
        let ephemeral = storage.capture_usage_update_at_for_test(february);
        assert_eq!(ephemeral.period, january);
        assert_eq!(ephemeral.ephemeral_storage_byte_seconds_delta, 3);
        assert_eq!(ephemeral.ephemeral_storage_byte_nanoseconds_remainder, 0);
        let ephemeral_remainder = storage.capture_usage_update_at_for_test(february);
        assert_eq!(ephemeral_remainder.period, january);
        assert_eq!(ephemeral_remainder.ephemeral_storage_byte_seconds_delta, 0);
        assert_eq!(
            ephemeral_remainder.ephemeral_storage_byte_nanoseconds_remainder,
            11
        );

        let memory = memory_entry(february, MonthlyUsageMode::HardLimit, 0);
        memory.record_memory_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement {
                units: 5,
                remainder: 13,
            },
        );

        let memory_update = memory.capture_usage_update_at_for_test(february);
        assert_eq!(memory_update.period, january);
        assert_eq!(memory_update.memory_gb_seconds_delta, 5);
        assert_eq!(memory_update.memory_byte_nanoseconds_remainder, 0);
        let memory_remainder = memory.capture_usage_update_at_for_test(february);
        assert_eq!(memory_remainder.period, january);
        assert_eq!(memory_remainder.memory_gb_seconds_delta, 0);
        assert_eq!(memory_remainder.memory_byte_nanoseconds_remainder, 13);
    }

    #[test]
    fn oversized_late_settlements_are_fully_drained_with_one_remainder() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let oversized = i64::MAX as u128 + 5;

        let storage = storage_entry(february, MonthlyUsageMode::AllowOverage, 0, 0);
        storage.record_resource_settlement_for_period(
            AgentMode::Ephemeral,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: oversized,
                remainder: 17,
            },
        );
        let first = storage.capture_usage_update_at(february, i64::MAX).unwrap();
        let second = storage.capture_usage_update_at(february, i64::MAX).unwrap();
        let remainder = storage.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(first.update.period, january);
        assert_eq!(first.update.ephemeral_storage_byte_seconds_delta, i64::MAX);
        assert_eq!(second.update.ephemeral_storage_byte_seconds_delta, 5);
        assert_eq!(remainder.update.ephemeral_storage_byte_seconds_delta, 0);
        assert_eq!(first.update.ephemeral_storage_byte_nanoseconds_remainder, 0);
        assert_eq!(
            second.update.ephemeral_storage_byte_nanoseconds_remainder,
            0
        );
        assert_eq!(
            remainder
                .update
                .ephemeral_storage_byte_nanoseconds_remainder,
            17
        );

        storage.record_resource_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: oversized,
                remainder: 23,
            },
        );
        let first = storage.capture_usage_update_at(february, i64::MAX).unwrap();
        let second = storage.capture_usage_update_at(february, i64::MAX).unwrap();
        let remainder = storage.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(first.update.durable_storage_byte_seconds_delta, i64::MAX);
        assert_eq!(first.update.ephemeral_storage_byte_seconds_delta, 0);
        assert_eq!(second.update.durable_storage_byte_seconds_delta, 5);
        assert_eq!(remainder.update.durable_storage_byte_seconds_delta, 0);
        assert_eq!(
            remainder.update.durable_storage_byte_nanoseconds_remainder,
            23
        );
        assert!(
            storage
                .capture_usage_update_at(february, i64::MAX)
                .is_none()
        );

        let memory = memory_entry(february, MonthlyUsageMode::AllowOverage, 0);
        memory.record_memory_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement {
                units: oversized,
                remainder: 19,
            },
        );
        let first = memory.capture_usage_update_at(february, i64::MAX).unwrap();
        let second = memory.capture_usage_update_at(february, i64::MAX).unwrap();
        let remainder = memory.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(first.update.memory_gb_seconds_delta, i64::MAX);
        assert_eq!(first.durable_memory_gb_seconds_delta, i64::MAX);
        assert_eq!(second.update.memory_gb_seconds_delta, 5);
        assert_eq!(second.durable_memory_gb_seconds_delta, 5);
        assert_eq!(remainder.update.memory_gb_seconds_delta, 0);
        assert_eq!(remainder.update.memory_byte_nanoseconds_remainder, 19);
        assert!(memory.capture_usage_update_at(february, i64::MAX).is_none());
    }

    #[test]
    fn explicitly_rejected_prior_period_update_is_recaptured_exactly_once() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let entry = storage_entry(february, MonthlyUsageMode::HardLimit, 10, 10);
        entry.record_resource_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 7,
            },
        );

        let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        entry.begin_monthly_refresh(
            1,
            &captured.update,
            captured.durable_memory_gb_seconds_delta,
            captured.ephemeral_memory_gb_seconds_delta,
        );
        assert!(entry.apply_monthly_snapshot(
            1,
            storage_policy(february, MonthlyUsageMode::HardLimit, 10, 10),
            7,
            false,
        ));

        let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(retried.update.period, captured.update.period);
        assert_eq!(retried.update.monthly_usage_mode_revision, 7);
        assert_eq!(retried.update.durable_storage_byte_seconds_delta, 0);
        assert_eq!(retried.update.durable_storage_byte_nanoseconds_remainder, 7);
        let revision_state = entry.usage_revision_state.lock().unwrap();
        assert!(
            revision_state
                .monthly_policy
                .as_ref()
                .unwrap()
                .failed_delivery_usage
                .is_empty()
        );
        assert_eq!(
            entry.effective_storage_byte_nanoseconds_with_revision_state(
                &revision_state,
                AgentMode::Durable,
            ),
            10_000_000_000
        );
        drop(revision_state);

        entry.begin_monthly_refresh(
            2,
            &retried.update,
            retried.durable_memory_gb_seconds_delta,
            retried.ephemeral_memory_gb_seconds_delta,
        );
        assert!(entry.apply_monthly_snapshot(
            2,
            storage_policy(february, MonthlyUsageMode::HardLimit, 10, 10),
            7,
            true,
        ));
        assert!(entry.capture_usage_update_at(february, i64::MAX).is_none());
    }

    #[test]
    fn out_of_order_explicit_rejection_requeues_the_original_update() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let entry = memory_entry(february, MonthlyUsageMode::AllowOverage, 10);
        entry.record_memory_settlement_for_period(
            AgentMode::Ephemeral,
            january,
            ByteTimeSettlement {
                units: 0,
                remainder: 11,
            },
        );
        let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        entry.begin_monthly_refresh(
            1,
            &captured.update,
            captured.durable_memory_gb_seconds_delta,
            captured.ephemeral_memory_gb_seconds_delta,
        );
        entry.begin_monthly_refresh_for_test(2, 0);

        assert!(!entry.apply_monthly_snapshot(
            1,
            memory_policy(february, MonthlyUsageMode::AllowOverage, 10),
            7,
            false,
        ));
        let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(retried.update.period, january);
        assert_eq!(retried.update.monthly_usage_mode_revision, 7);
        assert_eq!(retried.update.memory_gb_seconds_delta, 0);
        assert_eq!(retried.update.memory_byte_nanoseconds_remainder, 11);
        assert_eq!(retried.ephemeral_memory_gb_seconds_delta, 0);
    }

    #[test]
    fn confirmed_server_failure_requeues_prior_period_byte_time() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let entry = storage_entry(february, MonthlyUsageMode::HardLimit, 10, 10);
        entry.record_resource_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 29,
            },
        );
        let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        entry.begin_monthly_refresh(
            1,
            &captured.update,
            captured.durable_memory_gb_seconds_delta,
            captured.ephemeral_memory_gb_seconds_delta,
        );

        assert!(entry.finish_confirmed_non_application(1));
        let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(retried.update.period, january);
        assert_eq!(retried.update.monthly_usage_mode_revision, 7);
        assert_eq!(
            retried.update.durable_storage_byte_nanoseconds_remainder,
            29
        );
    }

    #[test]
    fn duplicate_confirmed_non_application_is_not_latest_but_requeues_byte_time() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let entry = storage_entry(february, MonthlyUsageMode::HardLimit, 10, 10);
        entry.record_resource_settlement_for_period(
            AgentMode::Durable,
            january,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 31,
            },
        );
        let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        entry.begin_monthly_refresh(1, &captured.update, 0, 0);
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .as_mut()
            .unwrap()
            .settled_generation = 1;

        assert!(!entry.finish_confirmed_non_application(1));
        let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(retried.update.period, january);
        assert_eq!(
            retried.update.durable_storage_byte_nanoseconds_remainder,
            31
        );
        assert!(entry.capture_usage_update_at(february, i64::MAX).is_none());
    }

    #[test]
    fn out_of_order_confirmed_non_application_is_not_latest_but_requeues_byte_time() {
        let january = usage_period(2030, 1);
        let february = usage_period(2030, 2);
        let entry = memory_entry(february, MonthlyUsageMode::AllowOverage, 10);
        entry.record_memory_settlement_for_period(
            AgentMode::Ephemeral,
            january,
            ByteTimeSettlement {
                units: 0,
                remainder: 37,
            },
        );
        let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        entry.begin_monthly_refresh(1, &captured.update, 0, 0);
        entry.begin_monthly_refresh_for_test(2, 0);

        assert!(!entry.finish_confirmed_non_application(1));
        let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
        assert_eq!(retried.update.period, january);
        assert_eq!(retried.update.memory_byte_nanoseconds_remainder, 37);
        assert_eq!(retried.ephemeral_memory_gb_seconds_delta, 0);
    }

    #[test]
    fn revision_rotation_captures_each_usage_dimension_independently() {
        let compute = metered_entry_with_revision(1);
        assert!(compute.borrow_fuel(1));
        compute.update_policy_revision(2);
        let compute_update = compute
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(compute_update.update.monthly_policy_revision, 1);
        assert_eq!(compute_update.update.fuel_delta, 1);

        let memory = metered_entry_with_revision(1);
        memory.record_memory_gb_seconds(AgentMode::Durable, 2);
        memory.update_policy_revision(2);
        let memory_update = memory
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(memory_update.update.monthly_policy_revision, 1);
        assert_eq!(memory_update.update.memory_gb_seconds_delta, 2);

        let http = metered_entry_with_revision(1);
        assert!(http.record_http_call());
        http.update_policy_revision(2);
        let http_update = http
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(http_update.update.monthly_policy_revision, 1);
        assert_eq!(http_update.update.http_call_count_delta, 1);

        let rpc = metered_entry_with_revision(1);
        assert!(rpc.record_rpc_call());
        rpc.update_policy_revision(2);
        let rpc_update = rpc
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(rpc_update.update.monthly_policy_revision, 1);
        assert_eq!(rpc_update.update.rpc_call_count_delta, 1);
    }

    #[test]
    fn repeated_revision_rotations_preserve_fractional_settlement_remainders() {
        let entry = metered_entry_with_revision(3);
        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 0,
                remainder: BYTE_NANOSECONDS_PER_GB_SECOND - 1,
            },
        );
        entry.update_policy_revision(4);

        entry.record_resource_settlement(
            AgentMode::Durable,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 1_000_000_000 - 1,
            },
        );
        entry.update_policy_revision(5);

        entry.record_resource_settlement(
            AgentMode::Ephemeral,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 1_000_000_000 - 1,
            },
        );
        entry.update_policy_revision(6);

        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 0,
                remainder: 1,
            },
        );
        entry.record_resource_settlement(
            AgentMode::Durable,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 1,
            },
        );
        entry.record_resource_settlement(
            AgentMode::Ephemeral,
            ByteTimeSettlement::default(),
            ByteTimeSettlement {
                units: 0,
                remainder: 1,
            },
        );
        entry.update_policy_revision(7);

        let revision_3 = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(revision_3.update.monthly_policy_revision, 3);
        assert_eq!(
            revision_3.update.memory_byte_nanoseconds_remainder,
            BYTE_NANOSECONDS_PER_GB_SECOND as u64 - 1
        );
        assert_eq!(
            revision_3.update.durable_storage_byte_nanoseconds_remainder,
            0
        );
        assert_eq!(
            revision_3
                .update
                .ephemeral_storage_byte_nanoseconds_remainder,
            0
        );

        let revision_4 = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(revision_4.update.monthly_policy_revision, 4);
        assert_eq!(revision_4.update.memory_byte_nanoseconds_remainder, 0);
        assert_eq!(
            revision_4.update.durable_storage_byte_nanoseconds_remainder,
            1_000_000_000 - 1
        );
        assert_eq!(
            revision_4
                .update
                .ephemeral_storage_byte_nanoseconds_remainder,
            0
        );

        let revision_5 = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(revision_5.update.monthly_policy_revision, 5);
        assert_eq!(revision_5.update.memory_byte_nanoseconds_remainder, 0);
        assert_eq!(
            revision_5.update.durable_storage_byte_nanoseconds_remainder,
            0
        );
        assert_eq!(
            revision_5
                .update
                .ephemeral_storage_byte_nanoseconds_remainder,
            1_000_000_000 - 1
        );

        let revision_6 = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(revision_6.update.monthly_policy_revision, 6);
        assert_eq!(revision_6.update.memory_byte_nanoseconds_remainder, 1);
        assert_eq!(
            revision_6.update.durable_storage_byte_nanoseconds_remainder,
            1
        );
        assert_eq!(
            revision_6
                .update
                .ephemeral_storage_byte_nanoseconds_remainder,
            1
        );

        assert_eq!(
            revision_3.update.memory_byte_nanoseconds_remainder
                + revision_6.update.memory_byte_nanoseconds_remainder,
            BYTE_NANOSECONDS_PER_GB_SECOND as u64
        );
        assert_eq!(
            revision_4.update.durable_storage_byte_nanoseconds_remainder
                + revision_6.update.durable_storage_byte_nanoseconds_remainder,
            1_000_000_000
        );
        assert_eq!(
            revision_5
                .update
                .ephemeral_storage_byte_nanoseconds_remainder
                + revision_6
                    .update
                    .ephemeral_storage_byte_nanoseconds_remainder,
            1_000_000_000
        );
    }

    #[test]
    fn delayed_pre_disable_usage_keeps_its_accrual_revision() {
        let entry = metered_entry_with_revision(8);
        assert!(entry.borrow_fuel(100));
        entry.record_storage_byte_seconds(AgentMode::Durable, 10);

        entry.update_policy_revision(9);
        assert!(entry.borrow_fuel(200));
        entry.record_storage_byte_seconds(AgentMode::Durable, 20);

        let before_disable = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(before_disable.update.monthly_policy_revision, 8);
        assert_eq!(before_disable.update.fuel_delta, 100);
        assert_eq!(before_disable.update.durable_storage_byte_seconds_delta, 10);
        let after_disable = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(after_disable.update.monthly_policy_revision, 9);
        assert_eq!(after_disable.update.fuel_delta, 200);
        assert_eq!(after_disable.update.durable_storage_byte_seconds_delta, 20);
    }

    #[test]
    fn revision_refresh_does_not_relabel_in_flight_usage() {
        let entry = metered_entry_with_revision(12);
        assert!(entry.borrow_fuel(100));
        let in_flight = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();

        entry.update_policy_revision(13);
        assert!(entry.borrow_fuel(200));

        assert_eq!(in_flight.update.monthly_policy_revision, 12);
        let next = entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(next.update.monthly_policy_revision, 13);
        assert_eq!(next.update.fuel_delta, 200);
    }

    #[test]
    fn disabled_usage_dimensions_accumulate_and_export_zero() {
        let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
            10,
            20,
            30,
            40,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig::default(),
        );
        assert!(entry.account_usage_accumulator.is_none());
        entry.delta.store(17, Ordering::Release);
        entry.in_flight_delta.store(19, Ordering::Release);
        entry
            .in_flight_memory_gb_seconds_delta
            .store(23, Ordering::Release);
        assert!(entry.borrow_fuel(7));
        entry.return_fuel(3);
        entry.record_overdraft_debt(5);
        entry.record_memory_gb_seconds(AgentMode::Durable, 11);
        entry.record_storage_byte_seconds(AgentMode::Durable, 13);

        let captured = entry
            .capture_usage_update(0)
            .expect("stale limit refresh still produces an update");

        assert_eq!(entry.effective_fuel(), u64::MAX);
        assert_eq!(entry.delta.load(Ordering::Acquire), 17);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 19);
        assert_eq!(
            entry
                .in_flight_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            23
        );
        assert_eq!(captured.update.fuel_delta, 0);
        assert_eq!(captured.update.memory_gb_seconds_delta, 0);
        assert_eq!(captured.update.durable_storage_byte_seconds_delta, 0);
        assert_eq!(captured.update.ephemeral_storage_byte_seconds_delta, 0);
        assert!(!captured.update.metering.compute);
        assert!(!captured.update.metering.memory);
        assert!(!captured.update.metering.filesystem);
    }

    #[test]
    async fn unmanaged_filesystem_config_constructs_no_monthly_storage_path_but_keeps_disk_limit() {
        let mut config = GolemConfig::default();
        config.resource_usage_metering.filesystem = true;
        config.filesystem_storage.managed_xfs_root_dir = None;
        let metering = config.effective_resource_usage_metering();
        let entry = AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
            MonthlyResourcePolicy {
                period: AccountUsagePeriod::current(),
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 0,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 0,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            usize::MAX,
            usize::MAX,
            4_096,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            metering,
            0,
            0,
            0,
        );

        assert!(!metering.filesystem);
        assert!(
            entry
                .usage_revision_state
                .lock()
                .unwrap()
                .monthly_policy
                .is_none()
        );
        assert!(entry.account_usage_accumulator.is_none());
        entry.record_storage_byte_seconds(AgentMode::Durable, 10);
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

        assert_eq!(entry.max_disk_space_limit(), 4_096);
        entry.apply_agent_filesystem_limit(8_192).await.unwrap();
        assert_eq!(entry.max_disk_space_limit(), 8_192);
    }

    #[test]
    fn usage_update_reports_configured_metering_state() {
        let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: true,
                memory: false,
                filesystem: true,
            },
        );

        let captured = entry
            .capture_usage_update(0)
            .expect("stale limit refresh still produces an update");

        assert!(captured.update.metering.compute);
        assert!(!captured.update.metering.memory);
        assert!(captured.update.metering.filesystem);
    }

    #[test]
    fn usage_accumulation_respects_each_enabled_dimension() {
        for memory in [false, true] {
            for filesystem in [false, true] {
                let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
                    u64::MAX,
                    usize::MAX,
                    usize::MAX,
                    u64::MAX,
                    u64::MAX,
                    u64::MAX,
                    u64::MAX,
                    u64::MAX,
                    AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                    AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                    ResourceUsageMeteringConfig {
                        compute: false,
                        memory,
                        filesystem,
                    },
                );
                entry.record_memory_gb_seconds(AgentMode::Durable, 11);
                entry.record_storage_byte_seconds(AgentMode::Durable, 13);

                let (has_accumulator, has_memory, has_storage) = entry
                    .account_usage_accumulator
                    .as_ref()
                    .map_or((false, false, false), |accumulator| {
                        let usage = accumulator.lock().unwrap();
                        (true, usage.memory.is_some(), usage.storage.is_some())
                    });
                assert_eq!(has_accumulator, memory || filesystem);
                assert_eq!(has_memory, memory);
                assert_eq!(has_storage, filesystem);

                assert_eq!(
                    entry.memory_gb_seconds_delta(AgentMode::Durable),
                    if memory { 11 } else { 0 }
                );
                assert_eq!(
                    entry.durable_byte_seconds_delta(),
                    if filesystem { 13 } else { 0 }
                );
            }
        }
    }

    #[test]
    fn every_enabled_monthly_dimension_independently_controls_admission_in_all_meter_combinations()
    {
        let period = AccountUsagePeriod::current();
        for compute in [false, true] {
            for memory in [false, true] {
                for filesystem in [false, true] {
                    let metering = ResourceUsageMeteringConfig {
                        compute,
                        memory,
                        filesystem,
                    };
                    let cases = [
                        (
                            MonthlyResourceExhaustion::Compute,
                            compute,
                            AgentMode::Durable,
                        ),
                        (
                            MonthlyResourceExhaustion::Memory,
                            memory,
                            AgentMode::Durable,
                        ),
                        (
                            MonthlyResourceExhaustion::DurableStorage,
                            filesystem,
                            AgentMode::Durable,
                        ),
                        (
                            MonthlyResourceExhaustion::EphemeralStorage,
                            filesystem,
                            AgentMode::Ephemeral,
                        ),
                    ];

                    for (exhausted, enabled, agent_mode) in cases {
                        let mut policy = MonthlyResourcePolicy {
                            period,
                            mode: MonthlyUsageMode::HardLimit,
                            available_fuel: 1,
                            available_memory_gb_seconds: 1,
                            available_memory_byte_nanoseconds_remainder: 0,
                            available_durable_storage_byte_seconds: 1,
                            available_durable_storage_byte_nanoseconds_remainder: 0,
                            available_ephemeral_storage_byte_seconds: 1,
                            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
                        };
                        match exhausted {
                            MonthlyResourceExhaustion::Compute => policy.available_fuel = 0,
                            MonthlyResourceExhaustion::Memory => {
                                policy.available_memory_gb_seconds = 0
                            }
                            MonthlyResourceExhaustion::DurableStorage => {
                                policy.available_durable_storage_byte_seconds = 0
                            }
                            MonthlyResourceExhaustion::EphemeralStorage => {
                                policy.available_ephemeral_storage_byte_seconds = 0
                            }
                        }
                        let entry =
                            AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                                policy,
                                4_096,
                                usize::MAX,
                                8_192,
                                u64::MAX,
                                u64::MAX,
                                u64::MAX,
                                u64::MAX,
                                AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                                AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                                metering,
                                7,
                                7,
                                0,
                            );

                        let expected_capacity = enabled.then_some(exhausted);
                        assert_eq!(
                            entry.monthly_resource_capacity(agent_mode),
                            expected_capacity.map_or(Ok(()), Err),
                            "metering={metering:?}, exhausted={exhausted:?}, mode={agent_mode:?}"
                        );
                        let expected_admission = match (expected_capacity, agent_mode) {
                            (None, _) => MonthlyResourceAdmission::Admit,
                            (Some(_), AgentMode::Durable) => MonthlyResourceAdmission::Suspend,
                            (Some(exhaustion), AgentMode::Ephemeral) => {
                                MonthlyResourceAdmission::FailInvocation(exhaustion)
                            }
                        };
                        assert_eq!(
                            monthly_resource_admission(&entry, agent_mode),
                            expected_admission,
                            "metering={metering:?}, exhausted={exhausted:?}, mode={agent_mode:?}"
                        );

                        assert_eq!(entry.max_memory_limit(), 4_096);
                        assert_eq!(entry.max_disk_space_limit(), 8_192);
                    }

                    let accounting_entry =
                        AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                            MonthlyResourcePolicy {
                                period,
                                mode: MonthlyUsageMode::HardLimit,
                                available_fuel: 1,
                                available_memory_gb_seconds: 1,
                                available_memory_byte_nanoseconds_remainder: 0,
                                available_durable_storage_byte_seconds: 1,
                                available_durable_storage_byte_nanoseconds_remainder: 0,
                                available_ephemeral_storage_byte_seconds: 1,
                                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
                            },
                            4_096,
                            usize::MAX,
                            8_192,
                            u64::MAX,
                            u64::MAX,
                            u64::MAX,
                            u64::MAX,
                            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                            metering,
                            7,
                            7,
                            0,
                        );
                    assert!(accounting_entry.borrow_fuel(1));
                    accounting_entry.record_memory_gb_seconds(AgentMode::Durable, 1);
                    accounting_entry.record_storage_byte_seconds(AgentMode::Durable, 1);
                    accounting_entry.record_storage_byte_seconds(AgentMode::Ephemeral, 1);

                    let captured = accounting_entry
                        .capture_usage_update(0)
                        .expect("the zero refresh threshold includes every meter combination")
                        .update;
                    assert_eq!(captured.fuel_delta, i64::from(compute));
                    assert_eq!(captured.memory_gb_seconds_delta, i64::from(memory));
                    assert_eq!(
                        captured.durable_storage_byte_seconds_delta,
                        i64::from(filesystem)
                    );
                    assert_eq!(
                        captured.ephemeral_storage_byte_seconds_delta,
                        i64::from(filesystem)
                    );
                    assert_eq!(captured.metering.compute, compute);
                    assert_eq!(captured.metering.memory, memory);
                    assert_eq!(captured.metering.filesystem, filesystem);
                }
            }
        }
    }

    #[test]
    fn effective_fuel_with_zero_delta() {
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        assert_eq!(entry.effective_fuel(), 1000);
    }

    #[test]
    fn effective_fuel_subtracts_unsent_and_in_flight_usage() {
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(50));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(1, 50);
        assert!(entry.borrow_fuel(200));
        assert_eq!(entry.effective_fuel(), 750);
    }

    #[test]
    fn effective_fuel_applies_signed_refunds_with_saturation() {
        let entry = AtomicResourceEntry::new(100, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.delta.store(-200, Ordering::Release);
        assert_eq!(entry.effective_fuel(), 300);
    }

    #[test]
    fn effective_fuel_clamps_refund_overflow_to_u64_max() {
        let entry = AtomicResourceEntry::new(u64::MAX, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.delta.store(-1, Ordering::Release);
        assert_eq!(entry.effective_fuel(), u64::MAX);
    }

    #[test]
    fn borrow_fuel_succeeds_and_increases_delta() {
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(300));
        assert_eq!(entry.delta.load(Ordering::Acquire), 300);
        assert_eq!(entry.effective_fuel(), 700);
    }

    #[test]
    fn borrow_fuel_fails_when_effective_fuel_is_zero() {
        // fuel=0, delta=0 → effective=0; any non-zero borrow fails
        let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(!entry.borrow_fuel(1));
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    }

    #[test]
    fn borrow_fuel_uses_partial_final_capacity() {
        let entry = AtomicResourceEntry::new(100, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(101));
        assert_eq!(entry.delta.load(Ordering::Acquire), 100);
        assert!(!entry.borrow_fuel(1));
    }

    #[test]
    fn borrow_fuel_zero_amount_always_succeeds_without_touching_delta() {
        let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(0));
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    }

    #[test]
    fn borrow_fuel_exactly_at_effective_fuel_succeeds() {
        // Borrowing exactly effective_fuel must succeed
        let entry = AtomicResourceEntry::new(500, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(500));
        assert_eq!(entry.delta.load(Ordering::Acquire), 500);
    }

    #[test]
    fn borrow_fuel_one_over_effective_fuel_takes_the_exact_remainder() {
        let entry = AtomicResourceEntry::new(500, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(501));
        assert_eq!(entry.delta.load(Ordering::Acquire), 500);
    }

    #[test]
    fn return_fuel_decreases_delta() {
        // borrow 400 → delta = +400; return 100 unused → delta = 300
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.borrow_fuel(400);
        entry.return_fuel(100);
        assert_eq!(entry.delta.load(Ordering::Acquire), 300);
    }

    #[test]
    fn borrow_then_full_return_nets_delta_to_zero() {
        // borrow 500, return 500 (nothing consumed) → delta = 0
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.borrow_fuel(500);
        entry.return_fuel(500);
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    }

    #[test]
    fn return_fuel_does_not_panic_on_large_amount() {
        // delta at i64::MIN, return u64::MAX → saturates at i64::MIN, no panic
        let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.delta.store(i64::MIN, Ordering::Release);
        entry.return_fuel(u64::MAX);
        let _ = entry.delta.load(Ordering::Acquire);
    }

    #[test]
    fn record_overdraft_debt_increases_delta_by_actual_consumed_amount() {
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.record_overdraft_debt(2000);

        assert_eq!(entry.delta.load(Ordering::Acquire), 2000);
        assert_eq!(entry.effective_fuel(), 0);
    }

    #[test]
    fn hard_limit_zero_blocks_while_allow_overage_zero_borrows() {
        let period = AccountUsagePeriod::current();
        let hard = compute_entry(period, MonthlyUsageMode::HardLimit, 0);
        assert!(hard.monthly_resource_capacity(AgentMode::Durable).is_err());
        assert!(matches!(
            hard.borrow_fuel_with_revision(1),
            FuelBorrow::Exhausted { .. }
        ));

        let overage = compute_entry(period, MonthlyUsageMode::AllowOverage, 0);
        assert!(
            overage
                .monthly_resource_capacity(AgentMode::Durable)
                .is_ok()
        );
        assert_eq!(
            overage.borrow_fuel_with_revision(10),
            FuelBorrow::Borrowed {
                amount: 10,
                revision: 7,
                generation: 0,
                period,
            }
        );
        assert_eq!(overage.fuel_delta(), 10);
    }

    #[test]
    fn allow_overage_bypasses_direct_memory_and_storage_capacity_checks() {
        let period = AccountUsagePeriod::current();
        let memory = memory_entry(period, MonthlyUsageMode::AllowOverage, 0);
        assert!(
            memory
                .monthly_memory_and_storage_capacity(AgentMode::Durable)
                .is_ok()
        );

        let storage = storage_entry(period, MonthlyUsageMode::AllowOverage, 0, 0);
        assert!(
            storage
                .monthly_memory_and_storage_capacity(AgentMode::Durable)
                .is_ok()
        );
        assert!(
            storage
                .monthly_memory_and_storage_capacity(AgentMode::Ephemeral)
                .is_ok()
        );
    }

    #[test]
    fn exact_capacity_and_partial_final_borrow_are_atomic() {
        let entry = compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            15,
        );
        assert_eq!(
            entry.borrow_fuel_with_revision(10),
            FuelBorrow::Borrowed {
                amount: 10,
                revision: 7,
                generation: 0,
                period: AccountUsagePeriod::current(),
            }
        );
        assert_eq!(
            entry.borrow_fuel_with_revision(10),
            FuelBorrow::Borrowed {
                amount: 5,
                revision: 7,
                generation: 0,
                period: AccountUsagePeriod::current(),
            }
        );
        assert!(matches!(
            entry.borrow_fuel_with_revision(1),
            FuelBorrow::Exhausted { .. }
        ));
        assert_eq!(entry.fuel_delta(), 15);
    }

    #[test]
    fn every_local_delivery_state_reduces_hard_limit_capacity() {
        let unsent = compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        );
        assert!(unsent.borrow_fuel(10));
        assert_eq!(unsent.effective_fuel(), 90);

        let in_flight = compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        );
        assert!(in_flight.borrow_fuel(10));
        in_flight
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(in_flight.effective_fuel(), 90);

        let pending = compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        );
        assert!(pending.borrow_fuel(10));
        pending.update_policy_revision(8);
        assert_eq!(pending.effective_fuel(), 90);

        let failed = compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        );
        assert!(failed.borrow_fuel(10));
        failed
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        failed.begin_monthly_refresh_for_test(1, 10);
        assert!(failed.finish_ambiguous_transport_failure(1));
        assert_eq!(failed.effective_fuel(), 90);
        assert_eq!(failed.in_flight_delta.load(Ordering::Acquire), 0);
        assert!(
            failed
                .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
                .is_none(),
            "failed compute usage is retained only for enforcement"
        );
    }

    #[test]
    fn memory_hard_limit_clips_billable_usage_while_allow_overage_does_not() {
        let period = AccountUsagePeriod::current();
        let hard = memory_entry(period, MonthlyUsageMode::HardLimit, 10);
        hard.record_memory_gb_seconds(AgentMode::Durable, 14);

        assert_eq!(hard.effective_memory_gb_seconds(), 0);
        assert_eq!(
            hard.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        assert_eq!(
            hard.capture_usage_update_for_test().memory_gb_seconds_delta,
            10
        );
        assert_eq!(
            hard.usage_revision_state
                .lock()
                .unwrap()
                .monthly_policy
                .as_ref()
                .unwrap()
                .hard_limit_memory_overshoot_byte_nanoseconds
                .get(&(period, 7)),
            Some(&(4 * BYTE_NANOSECONDS_PER_GB_SECOND))
        );
        hard.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
        assert!(hard.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 2,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(
            hard.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        hard.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
        assert!(hard.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 5,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(hard.effective_memory_gb_seconds(), 1);

        let overage = memory_entry(period, MonthlyUsageMode::AllowOverage, 10);
        overage.record_memory_gb_seconds(AgentMode::Ephemeral, 14);

        assert!(overage.has_monthly_memory_capacity());
        assert!(
            overage
                .monthly_resource_capacity(AgentMode::Durable)
                .is_ok()
        );
        assert_eq!(
            overage
                .capture_usage_update_for_test()
                .memory_gb_seconds_delta,
            14
        );
    }

    #[test]
    fn storage_hard_limits_are_isolated_by_agent_mode() {
        let period = AccountUsagePeriod::current();
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);

        entry.record_storage_byte_seconds(AgentMode::Durable, 10);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::DurableStorage)
        );
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );

        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 20);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Ephemeral),
            Err(MonthlyResourceExhaustion::EphemeralStorage)
        );
        let captured = entry.capture_usage_update_for_test();
        assert_eq!(captured.durable_storage_byte_seconds_delta, 10);
        assert_eq!(captured.ephemeral_storage_byte_seconds_delta, 20);
    }

    #[test]
    fn storage_hard_limit_clips_billable_usage_and_preserves_exhaustion() {
        let period = AccountUsagePeriod::current();
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 1, u64::MAX);

        entry.record_storage_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 2,
                remainder: 500_000_000,
            },
        );

        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::DurableStorage)
        );
        let captured = entry.capture_usage_update_for_test();
        assert_eq!(captured.durable_storage_byte_seconds_delta, 1);
        assert_eq!(captured.durable_storage_byte_nanoseconds_remainder, 0);
        assert_eq!(
            entry
                .usage_revision_state
                .lock()
                .unwrap()
                .monthly_policy
                .as_ref()
                .unwrap()
                .hard_limit_durable_storage_overshoot_byte_nanoseconds
                .get(&(period, 7)),
            Some(&1_500_000_000)
        );
    }

    #[test]
    fn same_period_refresh_retains_storage_overshoot_for_admission() {
        let period = AccountUsagePeriod::current();
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 1, 1);
        entry.record_storage_byte_seconds(AgentMode::Durable, 2);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 2);

        assert!(entry.apply_monthly_snapshot_for_test(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 2,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 2,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
        ));
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::DurableStorage)
        );
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Ephemeral),
            Err(MonthlyResourceExhaustion::EphemeralStorage)
        );
    }

    #[test]
    fn storage_allow_overage_records_full_usage() {
        let entry = storage_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::AllowOverage,
            1,
            1,
        );

        entry.record_storage_byte_seconds(AgentMode::Durable, 3);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 4);

        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );
        let captured = entry.capture_usage_update_for_test();
        assert_eq!(captured.durable_storage_byte_seconds_delta, 3);
        assert_eq!(captured.ephemeral_storage_byte_seconds_delta, 4);
    }

    #[test]
    fn combined_resource_settlement_records_memory_and_storage_by_mode() {
        let entry = metered_entry_with_revision(7);

        entry.record_resource_settlement(
            AgentMode::Ephemeral,
            ByteTimeSettlement {
                units: 3,
                remainder: 250_000_000,
            },
            ByteTimeSettlement {
                units: 5,
                remainder: 750_000_000,
            },
        );

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 3);
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.ephemeral_byte_seconds_delta(), 5);
    }

    #[test]
    fn in_flight_storage_usage_remains_visible_to_hard_limit_enforcement() {
        let entry = storage_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            10,
            u64::MAX,
        );
        entry.record_storage_byte_seconds(AgentMode::Durable, 10);

        let captured = entry.capture_usage_update_for_test();

        assert_eq!(captured.durable_storage_byte_seconds_delta, 10);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::DurableStorage)
        );
    }

    #[test]
    fn concurrent_storage_settlements_cannot_bill_beyond_hard_limit() {
        let entry = Arc::new(storage_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
            u64::MAX,
        ));
        let recorders = (0..16)
            .map(|_| {
                let entry = Arc::clone(&entry);
                std::thread::spawn(move || {
                    entry.record_storage_byte_seconds(AgentMode::Durable, 10)
                })
            })
            .collect::<Vec<_>>();
        for recorder in recorders {
            recorder.join().unwrap();
        }

        assert_eq!(entry.durable_byte_seconds_delta(), 100);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::DurableStorage)
        );
        assert_eq!(
            entry
                .capture_usage_update_for_test()
                .durable_storage_byte_seconds_delta,
            100
        );
    }

    #[test]
    fn larger_storage_amount_recovers_admission_and_stale_revision_cannot_undo_it() {
        let period = AccountUsagePeriod::current();
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);
        entry.record_storage_byte_seconds(AgentMode::Durable, 10);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 20);
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_err()
        );

        assert!(entry.apply_monthly_snapshot_for_test(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 20,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 40,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );

        assert!(!entry.apply_monthly_snapshot_for_test(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 0,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 0,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );
    }

    #[test]
    fn new_month_recovers_storage_admission() {
        let period = AccountUsagePeriod::current();
        let next_period = if period.month == 12 {
            AccountUsagePeriod {
                year: period.year + 1,
                month: 1,
            }
        } else {
            AccountUsagePeriod {
                year: period.year,
                month: period.month + 1,
            }
        };
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);
        entry.record_storage_byte_seconds(AgentMode::Durable, 10);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 20);

        assert!(entry.apply_monthly_snapshot_for_test(
            1,
            MonthlyResourcePolicy {
                period: next_period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 10,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 20,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );
    }

    #[test]
    fn allow_overage_recovers_storage_admission_and_stale_revision_cannot_undo_it() {
        let period = AccountUsagePeriod::current();
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 0, 0);

        assert!(entry.apply_monthly_snapshot_for_test(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 0,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 0,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );

        assert!(!entry.apply_monthly_snapshot_for_test(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 0,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 0,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        assert!(
            entry
                .monthly_resource_capacity(AgentMode::Ephemeral)
                .is_ok()
        );
    }

    #[test]
    fn hard_limit_clips_memory_settlement_units_and_remainder() {
        let entry = memory_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            1,
        );
        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 2,
                remainder: BYTE_NANOSECONDS_PER_GB_SECOND / 2,
            },
        );

        let captured = entry.capture_usage_update_for_test();
        assert_eq!(captured.memory_gb_seconds_delta, 1);
        assert_eq!(captured.memory_byte_nanoseconds_remainder, 0);
    }

    #[test]
    fn fractional_memory_settlements_combine_before_hard_limit_enforcement() {
        let entry = memory_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            1,
        );
        let three_fifths = BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 5;

        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 0,
                remainder: three_fifths,
            },
        );
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
        entry.record_memory_settlement(
            AgentMode::Ephemeral,
            ByteTimeSettlement {
                units: 0,
                remainder: three_fifths,
            },
        );

        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        let captured = entry.capture_usage_update_for_test();
        assert_eq!(captured.memory_gb_seconds_delta, 1);
        assert_eq!(captured.memory_byte_nanoseconds_remainder, 0);
    }

    #[test]
    fn revisioned_fractional_memory_remains_visible_to_hard_limit_enforcement() {
        let entry = memory_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            1,
        );
        let three_quarters = BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 4;
        let one_quarter = BYTE_NANOSECONDS_PER_GB_SECOND / 4;

        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 0,
                remainder: three_quarters,
            },
        );
        entry.update_policy_revision(8);
        entry.record_memory_settlement(
            AgentMode::Ephemeral,
            ByteTimeSettlement {
                units: 0,
                remainder: three_quarters,
            },
        );

        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        entry.update_policy_revision(9);
        let revision_7 = entry.capture_usage_update_for_test();
        let revision_8 = entry.capture_usage_update_for_test();
        assert_eq!(revision_7.monthly_policy_revision, 7);
        assert_eq!(
            revision_7.memory_byte_nanoseconds_remainder,
            three_quarters as u64
        );
        assert_eq!(revision_8.monthly_policy_revision, 8);
        assert_eq!(
            revision_8.memory_byte_nanoseconds_remainder,
            one_quarter as u64
        );
    }

    #[test]
    fn confirmed_fractional_memory_preserves_exact_remaining_capacity() {
        let period = AccountUsagePeriod::current();
        let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 1);
        let three_quarters = BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 4;
        let one_quarter = BYTE_NANOSECONDS_PER_GB_SECOND / 4;

        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 0,
                remainder: three_quarters,
            },
        );
        entry.update_policy_revision(8);
        let captured = entry.capture_usage_update_for_test();
        entry.begin_monthly_refresh(1, &captured, 0, 0);

        assert!(entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 0,
                available_memory_byte_nanoseconds_remainder: one_quarter as u64,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
            true,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

        entry.record_memory_settlement(
            AgentMode::Ephemeral,
            ByteTimeSettlement {
                units: 0,
                remainder: one_quarter,
            },
        );
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
    }

    #[test]
    fn concurrent_memory_settlements_cannot_bill_beyond_hard_limit() {
        let entry = Arc::new(memory_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        ));
        let recorders = (0..16)
            .map(|_| {
                let entry = entry.clone();
                std::thread::spawn(move || entry.record_memory_gb_seconds(AgentMode::Durable, 10))
            })
            .collect::<Vec<_>>();
        for recorder in recorders {
            recorder.join().unwrap();
        }

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 100);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        assert_eq!(
            entry
                .capture_usage_update_for_test()
                .memory_gb_seconds_delta,
            100
        );
    }

    #[test]
    fn every_local_delivery_state_reduces_memory_hard_limit_capacity() {
        let period = AccountUsagePeriod::current();
        let unsent = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
        unsent.record_memory_gb_seconds(AgentMode::Durable, 10);
        assert_eq!(unsent.effective_memory_gb_seconds(), 90);

        let in_flight = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
        in_flight.record_memory_gb_seconds(AgentMode::Durable, 10);
        in_flight.capture_usage_update_for_test();
        assert_eq!(in_flight.effective_memory_gb_seconds(), 90);

        let pending = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
        pending.record_memory_gb_seconds(AgentMode::Durable, 10);
        pending.update_policy_revision(8);
        assert_eq!(pending.effective_memory_gb_seconds(), 90);

        let failed = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
        failed.record_memory_gb_seconds(AgentMode::Durable, 10);
        failed.capture_usage_update_for_test();
        failed.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
        assert!(failed.finish_ambiguous_transport_failure(1));
        assert_eq!(failed.effective_memory_gb_seconds(), 90);

        let stale = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
        stale.record_memory_gb_seconds(AgentMode::Durable, 10);
        stale.capture_usage_update_for_test();
        stale.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
        stale.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
        assert!(!stale.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 100,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(stale.effective_memory_gb_seconds(), 90);
    }

    #[test]
    fn memory_refresh_acknowledges_only_its_own_in_flight_usage() {
        let period = AccountUsagePeriod::current();
        let entry = memory_entry(period, MonthlyUsageMode::AllowOverage, 100);

        entry.record_memory_gb_seconds(AgentMode::Durable, 10);
        entry.capture_usage_update_for_test();
        entry.begin_monthly_refresh_with_memory_for_test(1, 0, 10);

        entry.record_memory_gb_seconds(AgentMode::Durable, 20);
        entry.capture_usage_update_for_test();
        entry.begin_monthly_refresh_with_memory_for_test(2, 0, 20);

        assert!(entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 70,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(
            entry
                .in_flight_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            10
        );
        assert_eq!(entry.effective_memory_gb_seconds(), 60);

        assert!(!entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(
            entry
                .in_flight_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(entry.effective_memory_gb_seconds(), 60);

        entry.begin_monthly_refresh_with_memory_for_test(3, 0, 0);
        assert!(entry.apply_monthly_snapshot(
            3,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 70,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(entry.effective_memory_gb_seconds(), 70);
    }

    #[test]
    fn monthly_admission_flushes_resident_memory_before_deciding() {
        let entry = Arc::new(memory_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            10,
        ));
        let flusher: Arc<dyn ResourceUsageFlusher> = Arc::new(MemoryUsageFlusher {
            entry: Arc::downgrade(&entry),
            amount: Mutex::new(Some(10)),
        });
        entry.register_resource_usage_flusher(Arc::downgrade(&flusher));

        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 10);
    }

    #[test]
    fn amount_mode_and_period_changes_restore_memory_admission() {
        let period = AccountUsagePeriod::current();
        let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 10);
        entry.record_memory_gb_seconds(AgentMode::Durable, 10);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );

        entry.begin_monthly_refresh_with_memory_for_test(1, 0, 0);
        assert!(entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 20,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

        entry.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
        assert!(entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 0,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
            true,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

        let failed = memory_entry(period, MonthlyUsageMode::HardLimit, 10);
        failed.record_memory_gb_seconds(AgentMode::Durable, 10);
        failed.capture_usage_update_for_test();
        failed.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
        assert!(failed.finish_ambiguous_transport_failure(1));
        assert_eq!(
            failed.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::Memory)
        );
        let next_period = if period.month == 12 {
            AccountUsagePeriod {
                year: period.year + 1,
                month: 1,
            }
        } else {
            AccountUsagePeriod {
                year: period.year,
                month: period.month + 1,
            }
        };
        failed.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
        assert!(failed.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period: next_period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 10,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
            true,
        ));
        assert!(failed.monthly_resource_capacity(AgentMode::Durable).is_ok());
    }

    #[test]
    fn memory_usage_advances_period_before_registry_refresh() {
        let current_period = AccountUsagePeriod::current();
        let previous_period = if current_period.month == 1 {
            AccountUsagePeriod {
                year: current_period.year - 1,
                month: 12,
            }
        } else {
            AccountUsagePeriod {
                year: current_period.year,
                month: current_period.month - 1,
            }
        };
        let entry = memory_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

        entry.record_memory_gb_seconds(AgentMode::Durable, 10);
        let captured = entry.capture_usage_update_for_test();

        assert_eq!(captured.period, current_period);
        assert_eq!(captured.memory_gb_seconds_delta, 10);
    }

    #[test]
    fn memory_disabled_entry_has_no_memory_gate_or_accounting() {
        let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: true,
                memory: false,
                filesystem: false,
            },
        );

        assert!(
            entry
                .usage_revision_state
                .lock()
                .unwrap()
                .monthly_policy
                .as_ref()
                .unwrap()
                .available_memory_byte_nanoseconds
                .is_none()
        );
        assert!(entry.account_usage_accumulator.is_none());
        entry.record_memory_gb_seconds(AgentMode::Durable, 10);
        assert_eq!(entry.effective_memory_gb_seconds(), u64::MAX);
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    }

    #[test]
    fn signed_refunds_reduce_unsent_and_failed_delivery_enforcement_usage() {
        let entry = compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        );
        assert!(entry.borrow_fuel(60));
        entry.return_fuel(20);
        assert_eq!(entry.effective_fuel(), 60);

        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(1, 40);
        assert!(entry.finish_ambiguous_transport_failure(1));
        assert_eq!(entry.effective_fuel(), 60);

        entry.return_fuel(20);
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(2, -20);
        assert!(entry.finish_ambiguous_transport_failure(2));
        assert_eq!(entry.effective_fuel(), 80);
    }

    #[test]
    fn concurrent_hard_limit_borrowers_cannot_exceed_capacity() {
        let entry = Arc::new(compute_entry(
            AccountUsagePeriod::current(),
            MonthlyUsageMode::HardLimit,
            100,
        ));
        let borrowers = (0..16)
            .map(|_| {
                let entry = entry.clone();
                std::thread::spawn(move || match entry.borrow_fuel_with_revision(10) {
                    FuelBorrow::Borrowed { amount, .. } => amount,
                    FuelBorrow::Exhausted { .. } => 0,
                })
            })
            .collect::<Vec<_>>();
        let borrowed = borrowers
            .into_iter()
            .map(|borrower| borrower.join().unwrap())
            .sum::<u64>();

        assert_eq!(borrowed, 100);
        assert_eq!(entry.fuel_delta(), 100);
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
    }

    #[test]
    fn stale_refresh_cannot_change_policy_or_revision() {
        let period = AccountUsagePeriod::current();
        let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 100);
        entry.begin_monthly_refresh_for_test(1, 0);
        assert!(entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: 0,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
            true,
        ));
        assert!(!entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 1_000,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert!(entry.borrow_fuel(20));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(2, 20);

        assert!(!entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 1_000,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            1,
            true,
        ));
        let revision_state = entry.usage_revision_state.lock().unwrap();
        assert_eq!(revision_state.current_policy_revision, 8);
        assert_eq!(
            revision_state.monthly_policy.as_ref().unwrap().mode,
            MonthlyUsageMode::AllowOverage
        );
        drop(revision_state);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 20);

        assert!(entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 0,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            9,
            true,
        ));
        entry.begin_monthly_refresh_for_test(3, 0);
        assert!(!entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            10,
            true,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
    }

    #[test]
    fn newer_generation_with_older_revision_cannot_restore_stale_storage_policy() {
        let period = AccountUsagePeriod::current();
        let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);
        entry.update_policy_revision(8);
        entry.record_storage_byte_seconds(AgentMode::Durable, 1);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 2);
        let captured = entry.capture_usage_update_for_test();
        entry.begin_monthly_refresh(1, &captured, 0, 0);

        assert!(!entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));

        let revision_state = entry.usage_revision_state.lock().unwrap();
        let gate = revision_state.monthly_policy.as_ref().unwrap();
        assert_eq!(revision_state.current_policy_revision, 8);
        assert_eq!(gate.mode, MonthlyUsageMode::HardLimit);
        assert_eq!(
            gate.available_durable_storage_byte_nanoseconds,
            Some(10_000_000_000)
        );
        assert_eq!(
            gate.available_ephemeral_storage_byte_nanoseconds,
            Some(20_000_000_000)
        );
        assert!(gate.in_flight_usage.is_empty());
        drop(revision_state);

        entry.record_storage_byte_seconds(AgentMode::Durable, 9);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 18);
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Durable),
            Err(MonthlyResourceExhaustion::DurableStorage)
        );
        assert_eq!(
            entry.monthly_resource_capacity(AgentMode::Ephemeral),
            Err(MonthlyResourceExhaustion::EphemeralStorage)
        );
    }

    #[test]
    fn refresh_acknowledges_only_its_own_in_flight_fuel() {
        let period = AccountUsagePeriod::current();
        let entry = compute_entry(period, MonthlyUsageMode::AllowOverage, 100);

        assert!(entry.borrow_fuel(10));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(1, 10);

        assert!(entry.borrow_fuel(20));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(2, 20);

        assert!(entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 70,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 10);
        assert_eq!(entry.effective_fuel(), 60);

        assert!(!entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.effective_fuel(), 60);

        entry.begin_monthly_refresh_for_test(3, 0);
        assert!(entry.apply_monthly_snapshot(
            3,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 70,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(entry.effective_fuel(), 70);
    }

    #[test]
    fn unapplied_registry_update_remains_charged_for_enforcement() {
        let period = AccountUsagePeriod::current();
        let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 100);
        assert!(entry.borrow_fuel(10));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(1, 10);

        assert!(entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 100,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            false,
        ));
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.effective_fuel(), 90);
        assert!(
            entry
                .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
                .is_none(),
            "unapplied compute usage is retained only for enforcement"
        );
    }

    #[test]
    fn amount_mode_and_period_changes_restore_compute_admission() {
        let period = AccountUsagePeriod::current();
        let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 10);
        assert!(entry.borrow_fuel(10));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());

        entry.begin_monthly_refresh_for_test(1, 0);
        assert!(entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 20,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

        entry.begin_monthly_refresh_for_test(2, 0);
        assert!(entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: 0,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            8,
            true,
        ));
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

        let failed = compute_entry(period, MonthlyUsageMode::HardLimit, 10);
        assert!(failed.borrow_fuel(10));
        failed
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        failed.begin_monthly_refresh_for_test(1, 10);
        assert!(failed.finish_ambiguous_transport_failure(1));
        assert!(
            failed
                .monthly_resource_capacity(AgentMode::Durable)
                .is_err()
        );
        let next_period = if period.month == 12 {
            AccountUsagePeriod {
                year: period.year + 1,
                month: 1,
            }
        } else {
            AccountUsagePeriod {
                year: period.year,
                month: period.month + 1,
            }
        };
        failed.begin_monthly_refresh_for_test(2, 0);
        assert!(failed.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period: next_period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 10,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert!(failed.monthly_resource_capacity(AgentMode::Durable).is_ok());
    }

    #[test]
    fn period_rollover_keeps_pre_refresh_usage_in_its_accrual_period() {
        let period = AccountUsagePeriod::current();
        let next_period = if period.month == 12 {
            AccountUsagePeriod {
                year: period.year + 1,
                month: 1,
            }
        } else {
            AccountUsagePeriod {
                year: period.year,
                month: period.month + 1,
            }
        };
        let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 100);
        assert!(entry.borrow_fuel(10));

        assert!(entry.apply_monthly_snapshot_for_test(
            1,
            MonthlyResourcePolicy {
                period: next_period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 100,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
        ));
        assert_eq!(entry.effective_fuel(), 100);

        let previous_period_usage = entry.capture_usage_update_for_test();
        assert_eq!(previous_period_usage.period, period);
        assert_eq!(previous_period_usage.fuel_delta, 10);

        assert!(entry.borrow_fuel(5));
        let current_period_usage = entry.capture_usage_update_for_test();
        assert_eq!(current_period_usage.period, next_period);
        assert_eq!(current_period_usage.fuel_delta, 5);
    }

    #[test]
    fn compute_usage_advances_period_before_registry_refresh() {
        let current_period = AccountUsagePeriod::current();
        let previous_period = if current_period.month == 1 {
            AccountUsagePeriod {
                year: current_period.year - 1,
                month: 12,
            }
        } else {
            AccountUsagePeriod {
                year: current_period.year,
                month: current_period.month - 1,
            }
        };
        let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

        assert_eq!(
            entry.borrow_fuel_with_revision(10),
            FuelBorrow::Borrowed {
                amount: 10,
                revision: 7,
                generation: 0,
                period: current_period,
            }
        );
        let captured = entry.capture_usage_update_for_test();
        assert_eq!(captured.period, current_period);
        assert_eq!(captured.fuel_delta, 10);
    }

    #[test]
    fn pre_refresh_rollover_borrows_share_the_remaining_capacity() {
        let current_period = AccountUsagePeriod::current();
        let previous_period = if current_period.month == 1 {
            AccountUsagePeriod {
                year: current_period.year - 1,
                month: 12,
            }
        } else {
            AccountUsagePeriod {
                year: current_period.year,
                month: current_period.month - 1,
            }
        };
        let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

        assert!(entry.borrow_fuel(60));
        assert_eq!(entry.effective_fuel(), 40);
        assert_eq!(
            entry.borrow_fuel_with_revision(60),
            FuelBorrow::Borrowed {
                amount: 40,
                revision: 7,
                generation: 0,
                period: current_period,
            }
        );
        assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
    }

    #[test]
    fn pre_refresh_rollover_in_flight_and_failed_usage_stays_charged() {
        let current_period = AccountUsagePeriod::current();
        let previous_period = if current_period.month == 1 {
            AccountUsagePeriod {
                year: current_period.year - 1,
                month: 12,
            }
        } else {
            AccountUsagePeriod {
                year: current_period.year,
                month: current_period.month - 1,
            }
        };
        let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

        assert!(entry.borrow_fuel(60));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        assert_eq!(entry.effective_fuel(), 40);
        entry.begin_monthly_refresh_for_test(1, 60);
        assert_eq!(entry.effective_fuel(), 40);

        assert!(entry.finish_ambiguous_transport_failure(1));
        assert_eq!(entry.effective_fuel(), 40);
        assert_eq!(
            entry.borrow_fuel_with_revision(60),
            FuelBorrow::Borrowed {
                amount: 40,
                revision: 7,
                generation: 1,
                period: current_period,
            }
        );

        let pending_entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);
        assert!(pending_entry.borrow_fuel(60));
        pending_entry.update_policy_revision(8);
        assert_eq!(pending_entry.effective_fuel(), 40);
    }

    #[test]
    fn newer_period_delivery_stays_charged_until_registry_reaches_that_period() {
        let current_period = AccountUsagePeriod::current();
        let previous_period = if current_period.month == 1 {
            AccountUsagePeriod {
                year: current_period.year - 1,
                month: 12,
            }
        } else {
            AccountUsagePeriod {
                year: current_period.year,
                month: current_period.month - 1,
            }
        };
        let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

        assert!(entry.borrow_fuel(60));
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .unwrap();
        entry.begin_monthly_refresh_for_test(1, 60);
        assert!(entry.apply_monthly_snapshot(
            1,
            MonthlyResourcePolicy {
                period: previous_period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 100,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(entry.effective_fuel(), 40);

        entry.begin_monthly_refresh_for_test(2, 0);
        assert!(entry.apply_monthly_snapshot(
            2,
            MonthlyResourcePolicy {
                period: current_period,
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 40,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
            true,
        ));
        assert_eq!(entry.effective_fuel(), 40);
    }

    #[test]
    fn compute_disabled_entry_has_no_compute_gate_or_accounting() {
        let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
            0,
            20,
            30,
            40,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: true,
            },
        );
        assert!(
            entry
                .usage_revision_state
                .lock()
                .unwrap()
                .monthly_policy
                .as_ref()
                .unwrap()
                .available_fuel
                .is_none()
        );
        assert_eq!(
            entry.borrow_fuel_with_revision(123),
            FuelBorrow::Borrowed {
                amount: 123,
                revision: 0,
                generation: 0,
                period: AccountUsagePeriod::current(),
            }
        );
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
        let captured = entry.capture_usage_update(0).unwrap();
        assert_eq!(captured.update.fuel_delta, 0);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
    }

    #[test]
    fn compute_disabled_entry_advances_shared_usage_attribution() {
        let period = AccountUsagePeriod::current();
        let next_period = AccountUsagePeriod {
            year: period.year + 1,
            month: 1,
        };
        let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
            0,
            20,
            30,
            40,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: true,
            },
        );

        assert!(entry.apply_monthly_snapshot_for_test(
            1,
            MonthlyResourcePolicy {
                period: next_period,
                mode: MonthlyUsageMode::AllowOverage,
                available_fuel: 0,
                available_memory_gb_seconds: u64::MAX,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            7,
        ));
        entry.record_resource_usage(AgentMode::Durable, 3, 5);
        let captured = entry.capture_usage_update_for_test();

        assert_eq!(captured.period, next_period);
        assert_eq!(captured.monthly_usage_mode_revision, 7);
        assert_eq!(captured.fuel_delta, 0);
        assert_eq!(captured.memory_gb_seconds_delta, 3);
        assert_eq!(captured.durable_storage_byte_seconds_delta, 5);
    }

    #[test]
    fn max_memory_limit_returns_stored_value() {
        let entry = AtomicResourceEntry::new(0, 65536, usize::MAX, u64::MAX, u64::MAX);
        assert_eq!(entry.max_memory_limit(), 65536);
    }

    #[test]
    fn last_refresh_secs_is_set_on_initialize() {
        let before = Utc::now().timestamp();
        let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, u64::MAX);
        let after = Utc::now().timestamp();
        let stored = entry.last_refresh_secs.load(Ordering::Acquire);
        assert!(stored >= before, "last_refresh_secs should be >= before");
        assert!(stored <= after, "last_refresh_secs should be <= after");
    }

    // -------------------------------------------------------------------------
    // AtomicResourceEntry — table element limit
    // -------------------------------------------------------------------------

    #[test]
    fn atomic_resource_entry_returns_table_elements_limit() {
        let entry = AtomicResourceEntry::new(1000, 65536, 500, u64::MAX, u64::MAX);
        assert_eq!(entry.max_table_elements_limit(), 500);
    }

    #[test]
    fn atomic_resource_entry_table_elements_independent_of_memory() {
        let entry = AtomicResourceEntry::new(0, 1024, 256, u64::MAX, u64::MAX);
        assert_eq!(entry.max_memory_limit(), 1024);
        assert_eq!(entry.max_table_elements_limit(), 256);
    }

    #[test]
    fn atomic_resource_entry_table_elements_usize_max_for_disabled() {
        let entry = AtomicResourceEntry::new(u64::MAX, usize::MAX, usize::MAX, u64::MAX, u64::MAX);
        assert_eq!(entry.max_table_elements_limit(), usize::MAX);
    }

    #[test]
    fn atomic_resource_entry_table_elements_zero() {
        let entry = AtomicResourceEntry::new(100, 4096, 0, u64::MAX, u64::MAX);
        assert_eq!(entry.max_table_elements_limit(), 0);
    }

    // -------------------------------------------------------------------------
    // AtomicResourceEntry — per-invocation limits
    // -------------------------------------------------------------------------

    #[test]
    fn new_with_invocation_limits_stores_http_limit() {
        let entry = AtomicResourceEntry::new_with_invocation_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            42,
            u64::MAX,
        );
        assert_eq!(entry.per_invocation_http_call_limit(), 42);
    }

    #[test]
    fn new_with_invocation_limits_stores_rpc_limit() {
        let entry = AtomicResourceEntry::new_with_invocation_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            99,
        );
        assert_eq!(entry.per_invocation_rpc_call_limit(), 99);
    }

    #[test]
    fn new_defaults_invocation_limits_to_max() {
        // AtomicResourceEntry::new (without invocation limits) must default to u64::MAX
        // so that workers using the old constructor are unaffected.
        let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, u64::MAX);
        assert_eq!(entry.per_invocation_http_call_limit(), u64::MAX);
        assert_eq!(entry.per_invocation_rpc_call_limit(), u64::MAX);
    }

    #[test]
    fn invocation_limits_can_be_updated_via_store() {
        let entry =
            AtomicResourceEntry::new_with_invocation_limits(500, 256, usize::MAX, u64::MAX, 10, 20);
        // Simulate a plan change: update limits via the atomic store
        entry
            .per_invocation_http_call_limit
            .store(50, Ordering::Release);
        entry
            .per_invocation_rpc_call_limit
            .store(100, Ordering::Release);
        assert_eq!(entry.per_invocation_http_call_limit(), 50);
        assert_eq!(entry.per_invocation_rpc_call_limit(), 100);
    }

    // -------------------------------------------------------------------------
    // AtomicResourceEntry — monthly HTTP/RPC call tracking
    // -------------------------------------------------------------------------

    #[test]
    fn update_last_known_limits_resets_syncing_and_refreshes_available() {
        // Simulate a batch response: syncing is cleared, available_from_server refreshed.
        let entry = AtomicResourceEntry::new_with_all_limits(
            0,
            0,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            5,
            5,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        // Simulate what send_batch does: move unsynced → syncing
        entry.syncing_http_calls.store(3, Ordering::Release);
        entry.syncing_rpc_calls.store(2, Ordering::Release);

        // Manually apply what update_last_known_limits does
        entry.syncing_http_calls.store(0, Ordering::Release);
        entry
            .available_http_calls_from_server
            .store(50, Ordering::Release);
        entry.syncing_rpc_calls.store(0, Ordering::Release);
        entry
            .available_rpc_calls_from_server
            .store(40, Ordering::Release);

        assert_eq!(entry.remaining_http_calls(), 50);
        assert_eq!(entry.remaining_rpc_calls(), 40);
    }

    #[test]
    fn record_http_call_returns_false_when_budget_exhausted() {
        // 0 available; any call should fail immediately.
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            0,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        assert!(
            !entry.record_http_call(),
            "call with 0 available should return false"
        );
    }

    #[test]
    fn record_http_call_exhausts_exactly_at_limit() {
        // 2 available; two calls succeed, third fails.
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            2,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        assert!(entry.record_http_call(), "first call should succeed");
        assert!(entry.record_http_call(), "second call should succeed");
        assert!(
            !entry.record_http_call(),
            "third call should fail — budget exhausted"
        );
    }

    #[test]
    fn record_rpc_call_decrements_remaining_rpc_calls() {
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            3,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        assert!(entry.record_rpc_call());
        assert_eq!(entry.remaining_rpc_calls(), 2);
    }

    #[test]
    fn record_rpc_call_returns_false_when_budget_exhausted() {
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            0,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        assert!(!entry.record_rpc_call());
    }

    #[test]
    fn http_and_rpc_budgets_are_independent() {
        // HTTP exhausted, RPC still available.
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            0,
            5,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        assert!(!entry.record_http_call(), "HTTP should be exhausted");
        assert!(entry.record_rpc_call(), "RPC should still be available");
    }

    #[test]
    fn unsynced_http_calls_accumulates_across_calls() {
        // Each record_http_call increments unsynced_http_calls by 1.
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            10,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        entry.record_http_call();
        entry.record_http_call();
        entry.record_http_call();
        // 3 calls made locally, not yet synced
        assert_eq!(entry.unsynced_http_calls.load(Ordering::Acquire), 3);
        // remaining = 10 - 3 - 0 = 7
        assert_eq!(entry.remaining_http_calls(), 7);
    }

    #[test]
    fn moving_unsynced_to_syncing_preserves_remaining_http_calls() {
        // Start with 10 available and 3 unsynced local calls.
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            10,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );
        entry.unsynced_http_calls.store(3, Ordering::Release);
        assert_eq!(entry.remaining_http_calls(), 7);

        // Simulate send_batch's transfer: unsynced -> syncing.
        let moved = entry.unsynced_http_calls.swap(0, Ordering::AcqRel);
        entry
            .syncing_http_calls
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                Some(c.saturating_add(moved))
            })
            .ok();

        // Remaining must stay unchanged while the batch is in flight.
        assert_eq!(entry.remaining_http_calls(), 7);
    }

    #[test]
    fn clearing_syncing_does_not_clear_new_unsynced_calls() {
        let entry = AtomicResourceEntry::new_with_all_limits(
            1000,
            512,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            10,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );

        // One call is included in the in-flight batch.
        entry.unsynced_http_calls.store(1, Ordering::Release);
        let moved = entry.unsynced_http_calls.swap(0, Ordering::AcqRel);
        entry
            .syncing_http_calls
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                Some(c.saturating_add(moved))
            })
            .ok();

        // While request is in-flight, two new local calls are recorded.
        entry.unsynced_http_calls.fetch_add(2, Ordering::AcqRel);

        // Simulate successful response handling: clear syncing and refresh available.
        entry.syncing_http_calls.store(0, Ordering::Release);
        entry
            .available_http_calls_from_server
            .store(100, Ordering::Release);

        // New unsynced calls made during in-flight period must be preserved.
        assert_eq!(entry.unsynced_http_calls.load(Ordering::Acquire), 2);
        assert_eq!(entry.remaining_http_calls(), 98);
    }

    #[test]
    async fn batch_success_refreshes_http_rpc_available_counts() {
        // After a successful send_batch, http_calls and rpc_calls should be
        // updated from the server response and in_flight cleared.
        let id = AccountId::SYSTEM;
        let mock = Arc::new(MockRegistryService::new(1000, 512));

        // Prime the entry with 5 available HTTP and 3 available RPC.
        mock.set_get_limits_response(ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: 5,
            available_rpc_calls: 3,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        });

        let svc = make_grpc(mock.clone());
        let entry: Arc<AtomicResourceEntry> = svc.initialize_account(id).await.unwrap();

        // Record some calls to build up deltas.
        entry.record_http_call();
        entry.record_http_call();
        entry.record_rpc_call();

        // Server will respond with fresh counts.
        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(1000),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: 50,
                available_rpc_calls: 40,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        svc.send_batch(0).await;

        // After batch success remaining must reflect the server's fresh available count.
        assert_eq!(entry.remaining_http_calls(), 50);
        assert_eq!(entry.remaining_rpc_calls(), 40);
        // syncing buckets cleared, unsynced also zero (were swapped to syncing)
        assert_eq!(entry.syncing_http_calls.load(Ordering::Acquire), 0);
        assert_eq!(entry.syncing_rpc_calls.load(Ordering::Acquire), 0);
        assert_eq!(entry.unsynced_http_calls.load(Ordering::Acquire), 0);
        assert_eq!(entry.unsynced_rpc_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    async fn batch_failure_clears_http_rpc_in_flight_without_double_counting() {
        // On batch failure the in-flight deltas must be cleared so the next
        // tick doesn't double-count them.
        let id = account_id();
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        mock.set_get_limits_response(ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: 10,
            available_rpc_calls: 10,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        });
        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(1000),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: 10,
                available_rpc_calls: 10,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));
        mock.set_batch_update_error();

        let svc = make_grpc(mock.clone());
        let entry: Arc<AtomicResourceEntry> = svc.initialize_account(id).await.unwrap();
        entry.record_http_call();
        entry.record_rpc_call();

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        // After the batch error the syncing buckets must be zeroed.
        assert_eq!(
            entry.syncing_http_calls.load(Ordering::Acquire),
            0,
            "syncing_http_calls should be cleared on error"
        );
        assert_eq!(
            entry.syncing_rpc_calls.load(Ordering::Acquire),
            0,
            "syncing_rpc_calls should be cleared on error"
        );
    }

    #[test]
    async fn resource_limits_disabled_returns_max_table_elements() {
        let disabled = ResourceLimitsDisabled;
        let entry = disabled
            .initialize_account(AccountId::SYSTEM)
            .await
            .expect("initialize_account should succeed");
        assert_eq!(entry.max_table_elements_limit(), usize::MAX);
    }

    // -------------------------------------------------------------------------
    // AtomicResourceEntry — concurrent agent limit
    // -------------------------------------------------------------------------

    #[test]
    fn concurrent_agent_limit_defaults_to_max_when_passing_u64_max() {
        let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, u64::MAX);
        assert_eq!(entry.max_concurrent_agents_per_executor(), u64::MAX);
    }

    #[test]
    fn concurrent_agent_limit_is_stored_from_new() {
        let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, 5);
        assert_eq!(entry.max_concurrent_agents_per_executor(), 5);
    }

    #[test]
    fn concurrent_agent_limit_zero_is_stored_correctly() {
        let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, 0);
        assert_eq!(entry.max_concurrent_agents_per_executor(), 0);
    }

    #[test]
    fn concurrent_agent_limit_can_be_updated_atomically() {
        let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, 5);
        entry.set_max_concurrent_agents_per_executor(10);
        assert_eq!(entry.max_concurrent_agents_per_executor(), 10);
    }

    #[test]
    fn concurrent_agent_limit_is_independent_of_other_fields() {
        let entry = AtomicResourceEntry::new(500, 1024, 256, 4096, 7);
        assert_eq!(entry.max_concurrent_agents_per_executor(), 7);
        assert_eq!(entry.effective_fuel(), 500);
        assert_eq!(entry.max_memory_limit(), 1024);
        assert_eq!(entry.max_table_elements_limit(), 256);
        assert_eq!(entry.max_disk_space_limit(), 4096);
    }

    // -------------------------------------------------------------------------
    // ResourceLimitsGrpc
    // -------------------------------------------------------------------------

    struct MockRegistryService {
        get_limits_result: Mutex<Result<ServiceResourceLimits, RegistryServiceError>>,
        batch_update_result: Mutex<Result<AccountResourceLimits, RegistryServiceError>>,
        delayed_batch_updates: Mutex<VecDeque<DelayedBatchUpdate>>,
        last_batch_updates: Mutex<HashMap<AccountId, ResourceUsageUpdate>>,
        batch_update_calls: AtomicUsize,
    }

    struct DelayedBatchUpdate {
        result: AccountResourceLimits,
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    impl MockRegistryService {
        fn new(available_fuel: u64, max_memory: u64) -> Self {
            Self {
                get_limits_result: Mutex::new(Ok(ServiceResourceLimits {
                    monthly_policy: monthly_policy(available_fuel),
                    max_memory_per_worker: max_memory,
                    max_table_elements_per_worker: u64::MAX,
                    max_disk_space_per_worker: u64::MAX,
                    per_invocation_http_call_limit: u64::MAX,
                    per_invocation_rpc_call_limit: u64::MAX,
                    available_http_calls: u64::MAX,
                    available_rpc_calls: u64::MAX,
                    max_concurrent_agents_per_executor: u64::MAX,
                    oplog_writes_per_second: u64::MAX,
                    usage_update_applied: true,
                    monthly_usage_mode_revision: 0,
                    monthly_policy_revision: 0,
                })),
                batch_update_result: Mutex::new(Ok(AccountResourceLimits(HashMap::new()))),
                delayed_batch_updates: Mutex::new(VecDeque::new()),
                last_batch_updates: Mutex::new(HashMap::new()),
                batch_update_calls: AtomicUsize::new(0),
            }
        }

        fn set_get_limits_response(&self, limits: ServiceResourceLimits) {
            *self.get_limits_result.lock().unwrap() = Ok(limits);
        }

        fn set_get_limits_error(&self) {
            *self.get_limits_result.lock().unwrap() = Err(
                RegistryServiceError::InternalServerError("mock error".into()),
            );
        }

        fn set_batch_update_response(&self, limits: AccountResourceLimits) {
            *self.batch_update_result.lock().unwrap() = Ok(limits);
        }

        fn set_batch_update_error(&self) {
            *self.batch_update_result.lock().unwrap() = Err(
                RegistryServiceError::InternalServerError("mock batch error".into()),
            );
        }

        fn delay_next_batch_update(
            &self,
            result: AccountResourceLimits,
        ) -> (Arc<Semaphore>, Arc<Semaphore>) {
            let started = Arc::new(Semaphore::new(0));
            let release = Arc::new(Semaphore::new(0));
            self.delayed_batch_updates
                .lock()
                .unwrap()
                .push_back(DelayedBatchUpdate {
                    result,
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                });
            (started, release)
        }

        fn last_batch_update(&self, account_id: AccountId) -> ResourceUsageUpdate {
            *self
                .last_batch_updates
                .lock()
                .unwrap()
                .get(&account_id)
                .unwrap()
        }
    }

    #[async_trait]
    impl RegistryService for MockRegistryService {
        async fn authenticate_token(
            &self,
            _token: &TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _account_id: AccountId,
        ) -> Result<ServiceResourceLimits, RegistryServiceError> {
            self.get_limits_result
                .lock()
                .unwrap()
                .clone()
                .map_err(|e| RegistryServiceError::InternalServerError(e.to_string()))
        }

        async fn update_worker_connection_limit(
            &self,
            _account_id: AccountId,
            _agent_id: &AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_resource_usage(
            &self,
            updates: HashMap<AccountId, ResourceUsageUpdate>,
        ) -> Result<AccountResourceLimits, RegistryServiceError> {
            self.batch_update_calls.fetch_add(1, Ordering::AcqRel);
            *self.last_batch_updates.lock().unwrap() = updates;
            let delayed = self.delayed_batch_updates.lock().unwrap().pop_front();
            if let Some(delayed) = delayed {
                delayed.started.add_permits(1);
                delayed.release.acquire().await.unwrap().forget();
                return Ok(delayed.result);
            }
            self.batch_update_result
                .lock()
                .unwrap()
                .clone()
                .map_err(|e| RegistryServiceError::InternalServerError(e.to_string()))
        }

        async fn download_component(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_component_metadata(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_deployed_component_metadata(
            &self,
            _component_id: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_deployed_component_revisions(
            &self,
            _component_id: ComponentId,
        ) -> Result<Vec<Component>, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_component(
            &self,
            _resolving_account_id: AccountId,
            _resolving_application_id: ApplicationId,
            _resolving_environment_id: EnvironmentId,
            _component_slug: &str,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_agent_types(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_agent_type(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
            _name: &AgentTypeName,
        ) -> Result<RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_by_names(
            &self,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
            _deployment_revision: Option<DeploymentRevision>,
            _owner_account_email: Option<&str>,
            _auth_ctx: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_routes_for_domain(
            &self,
            _domain: &Domain,
        ) -> Result<CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_compiled_mcps_for_domain(
            &self,
            _domain: &Domain,
        ) -> Result<CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_current_environment_state(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<EnvironmentState, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_agent_secret_revision(
            &self,
            _environment_id: EnvironmentId,
            _agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
            _path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
            _revision: golem_common::model::agent_secret::AgentSecretRevision,
        ) -> Result<
            Option<golem_service_base::model::agent_secret::AgentSecret>,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn get_resource_definition_by_id(
            &self,
            _resource_definition_id: ResourceDefinitionId,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_name(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn subscribe_registry_invalidations(
            &self,
            _last_seen_event_id: Option<u64>,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                golem_common::model::agent::RegistryInvalidationEvent,
                                RegistryServiceError,
                            >,
                        > + Send,
                >,
            >,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn run_registry_invalidation_event_subscriber(
            &self,
            _service_name: &'static str,
            _shutdown_token: Option<tokio_util::sync::CancellationToken>,
            _handler: std::sync::Arc<
                dyn golem_service_base::clients::registry::RegistryInvalidationHandler,
            >,
        ) {
            unimplemented!()
        }
    }

    fn account_id() -> AccountId {
        AccountId(Uuid::new_v4())
    }

    // Threshold used in tests that want stale idle accounts to be picked up.
    const STALE_THRESHOLD_SECS: i64 = 300;
    // Threshold used in tests that want idle accounts to never be picked up.
    const NO_IDLE_REFRESH_THRESHOLD_SECS: i64 = i64::MAX;

    fn make_grpc(mock: Arc<MockRegistryService>) -> Arc<ResourceLimitsGrpc> {
        // Tests drive send_batch manually, without a competing background updater.
        Arc::new(ResourceLimitsGrpc {
            client: mock,
            entries: scc::HashMap::new(),
            metering: ResourceUsageMeteringConfig::all_enabled(),
            refresh_generation: AtomicU64::new(1),
        })
    }

    fn service_limits(
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
        max_memory_per_worker: u64,
        max_disk_space_per_worker: u64,
    ) -> ServiceResourceLimits {
        ServiceResourceLimits {
            monthly_policy,
            max_memory_per_worker,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: u64::MAX,
            usage_update_applied: true,
            monthly_usage_mode_revision,
            monthly_policy_revision: monthly_usage_mode_revision,
        }
    }

    fn account_limits(
        account_id: AccountId,
        limits: ServiceResourceLimits,
    ) -> AccountResourceLimits {
        AccountResourceLimits(HashMap::from([(account_id, limits)]))
    }

    fn assert_monthly_resource_admission(
        entry: &AtomicResourceEntry,
        durable: MonthlyResourceAdmission,
        ephemeral: MonthlyResourceAdmission,
    ) {
        assert_eq!(
            monthly_resource_admission(entry, AgentMode::Durable),
            durable
        );
        assert_eq!(
            monthly_resource_admission(entry, AgentMode::Ephemeral),
            ephemeral
        );
    }

    #[test]
    async fn initialize_account_fetches_limits_from_registry() {
        let mock = Arc::new(MockRegistryService::new(5000, 1024));
        let svc = make_grpc(mock);
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();

        assert_eq!(entry.effective_fuel(), 5000);
        assert_eq!(entry.max_memory_limit(), 1024);
    }

    #[test]
    async fn all_disabled_production_service_skips_monthly_work_and_keeps_per_agent_limits() {
        let account_id = account_id();
        let mock = Arc::new(MockRegistryService::new(0, 4_096));
        mock.set_get_limits_response(service_limits(
            MonthlyResourcePolicy {
                available_memory_gb_seconds: 0,
                available_durable_storage_byte_seconds: 0,
                available_ephemeral_storage_byte_seconds: 0,
                ..monthly_policy(0)
            },
            0,
            4_096,
            8_192,
        ));
        let token = CancellationToken::new();
        token.cancel();
        let client: Arc<dyn RegistryService> = mock.clone();
        let svc = ResourceLimitsGrpc::new(
            client,
            Duration::from_secs(3600),
            Duration::from_secs(300),
            ResourceUsageMeteringConfig::default(),
            token,
        );

        let entry = svc.initialize_account(account_id).await.unwrap();
        assert!(
            entry
                .usage_revision_state
                .lock()
                .unwrap()
                .monthly_policy
                .is_none()
        );
        assert!(entry.account_usage_accumulator.is_none());
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Admit,
            MonthlyResourceAdmission::Admit,
        );
        assert!(entry.borrow_fuel(u64::MAX));
        entry.record_memory_gb_seconds(AgentMode::Durable, i64::MAX);
        entry.record_storage_byte_seconds(AgentMode::Durable, i64::MAX);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, i64::MAX);

        let memory_target = Arc::new(RecordingMemoryLimitTarget {
            enforced: AtomicU64::new(0),
        });
        let memory_target_dyn: Arc<dyn AgentMemoryLimitTarget> = memory_target.clone();
        entry.register_agent_memory_limit_target(Arc::downgrade(&memory_target_dyn));
        let observed_disk_limits = Arc::new(Mutex::new(Vec::new()));
        let _filesystem_registration = entry.register_agent_filesystem_limit_target(
            OwnedAgentId::new(
                EnvironmentId(Uuid::new_v4()),
                &AgentId {
                    component_id: ComponentId(Uuid::new_v4()),
                    agent_id: "all-disabled-managed-xfs".to_string(),
                },
            ),
            {
                let observed_disk_limits = Arc::clone(&observed_disk_limits);
                move |limit| {
                    let observed_disk_limits = Arc::clone(&observed_disk_limits);
                    Box::pin(async move {
                        observed_disk_limits.lock().unwrap().push(limit);
                        Ok(())
                    })
                }
            },
        );

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(mock.batch_update_calls.load(Ordering::Acquire), 0);
        assert_eq!(entry.max_memory_limit(), 4_096);
        assert_eq!(entry.max_disk_space_limit(), 8_192);
        entry.update_memory_limit(2_048);
        entry.apply_agent_filesystem_limit(4_096).await.unwrap();
        assert_eq!(entry.max_memory_limit(), 2_048);
        assert_eq!(entry.max_disk_space_limit(), 4_096);
        assert_eq!(memory_target.enforced.load(Ordering::Acquire), 2_048);
        assert_eq!(*observed_disk_limits.lock().unwrap(), vec![4_096]);
    }

    #[test]
    async fn production_refresh_updates_shared_entry_in_both_directions() {
        let account_id = account_id();
        let mock = Arc::new(MockRegistryService::new(0, usize::MAX as u64));
        let svc = make_grpc(Arc::clone(&mock));
        let entry = svc.initialize_account(account_id).await.unwrap();
        let shared_entry = svc.initialize_account(account_id).await.unwrap();
        assert!(Arc::ptr_eq(&entry, &shared_entry));
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
        );

        mock.set_batch_update_response(account_limits(
            account_id,
            service_limits(monthly_policy(1), 0, usize::MAX as u64, u64::MAX),
        ));
        svc.send_batch(0).await;
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Admit,
            MonthlyResourceAdmission::Admit,
        );

        mock.set_batch_update_response(account_limits(
            account_id,
            service_limits(monthly_policy(0), 0, usize::MAX as u64, u64::MAX),
        ));
        svc.send_batch(0).await;
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
        );
    }

    #[test]
    async fn production_refresh_updates_existing_per_agent_limit_targets() {
        let account_id = account_id();
        let mock = Arc::new(MockRegistryService::new(1, 8_192));
        mock.set_get_limits_response(service_limits(monthly_policy(1), 0, 8_192, 16_384));
        let svc = make_grpc(Arc::clone(&mock));
        let entry = svc.initialize_account(account_id).await.unwrap();
        let memory_target = Arc::new(RecordingMemoryLimitTarget {
            enforced: AtomicU64::new(0),
        });
        let memory_target_dyn: Arc<dyn AgentMemoryLimitTarget> = memory_target.clone();
        entry.register_agent_memory_limit_target(Arc::downgrade(&memory_target_dyn));
        let observed_disk_limits = Arc::new(Mutex::new(Vec::new()));
        let _filesystem_registration = entry.register_agent_filesystem_limit_target(
            OwnedAgentId::new(
                EnvironmentId(Uuid::new_v4()),
                &AgentId {
                    component_id: ComponentId(Uuid::new_v4()),
                    agent_id: "refresh-existing-limit-targets".to_string(),
                },
            ),
            {
                let observed_disk_limits = Arc::clone(&observed_disk_limits);
                move |limit| {
                    let observed_disk_limits = Arc::clone(&observed_disk_limits);
                    Box::pin(async move {
                        observed_disk_limits.lock().unwrap().push(limit);
                        Ok(())
                    })
                }
            },
        );
        mock.set_batch_update_response(account_limits(
            account_id,
            service_limits(monthly_policy(1), 0, 4_096, 8_192),
        ));

        svc.send_batch(0).await;

        assert_eq!(entry.max_memory_limit(), 4_096);
        assert_eq!(entry.max_disk_space_limit(), 8_192);
        assert_eq!(memory_target.enforced.load(Ordering::Acquire), 4_096);
        assert_eq!(*observed_disk_limits.lock().unwrap(), vec![8_192]);
    }

    #[test]
    async fn delayed_older_refresh_cannot_restore_admission() {
        let account_id = account_id();
        let mock = Arc::new(MockRegistryService::new(1, usize::MAX as u64));
        let svc = make_grpc(Arc::clone(&mock));
        let entry = svc.initialize_account(account_id).await.unwrap();

        let (older_started, release_older) = mock.delay_next_batch_update(account_limits(
            account_id,
            service_limits(monthly_policy(1), 0, usize::MAX as u64, u64::MAX),
        ));
        let older_refresh = {
            let svc = Arc::clone(&svc);
            tokio::spawn(async move { svc.send_batch(0).await })
        };
        older_started.acquire().await.unwrap().forget();

        mock.set_batch_update_response(account_limits(
            account_id,
            service_limits(monthly_policy(0), 0, usize::MAX as u64, u64::MAX),
        ));
        svc.send_batch(0).await;
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
        );

        release_older.add_permits(1);
        older_refresh.await.unwrap();
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
        );
    }

    #[test]
    async fn owner_overage_refresh_recovers_durable_and_ephemeral_admission() {
        let account_id = account_id();
        let mock = Arc::new(MockRegistryService::new(0, usize::MAX as u64));
        let svc = make_grpc(Arc::clone(&mock));
        let entry = svc.initialize_account(account_id).await.unwrap();
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
        );

        let mut owner_overage = monthly_policy(0);
        owner_overage.mode = MonthlyUsageMode::AllowOverage;
        mock.set_batch_update_response(account_limits(
            account_id,
            service_limits(owner_overage, 1, usize::MAX as u64, u64::MAX),
        ));

        svc.send_batch(0).await;

        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Admit,
            MonthlyResourceAdmission::Admit,
        );
    }

    #[test]
    async fn initialize_account_same_account_returns_shared_entry() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock);
        let id = account_id();

        let entry1 = svc.initialize_account(id).await.unwrap();
        let entry2 = svc.initialize_account(id).await.unwrap();

        // Both arcs must point to the exact same allocation
        assert!(Arc::ptr_eq(&entry1, &entry2));
    }

    #[test]
    async fn initialize_account_different_accounts_return_different_entries() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock);

        let entry1 = svc.initialize_account(account_id()).await.unwrap();
        let entry2 = svc.initialize_account(account_id()).await.unwrap();

        assert!(!Arc::ptr_eq(&entry1, &entry2));
    }

    #[test]
    async fn initialize_account_propagates_registry_error() {
        let mock = Arc::new(MockRegistryService::new(0, 0));
        mock.set_get_limits_error();
        let svc = make_grpc(mock);

        let result = svc.initialize_account(account_id()).await;
        assert!(result.is_err());
    }

    /// One span per tick, not one for the lifetime of the batch loop.
    #[test]
    async fn send_batch_records_one_closed_span_when_it_sends_a_batch() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock);
        let entry = svc.initialize_account(account_id()).await.unwrap();
        entry.borrow_fuel(300);

        let recorder = crate::span_test_support::record_spans();
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        recorder.assert_closed_span("resource_limits_batch_update");
        recorder.assert_all_closed();
    }

    /// An idle tick is still spanned: discovering that there is nothing to send is
    /// itself work that can fail, and events recorded outside a span never reach
    /// the trace.
    #[test]
    async fn send_batch_records_one_closed_span_when_there_is_nothing_to_send() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock);
        let _ = svc.initialize_account(account_id()).await.unwrap();

        let recorder = crate::span_test_support::record_spans();
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        recorder.assert_closed_span("resource_limits_batch_update");
    }

    #[test]
    async fn send_batch_does_nothing_when_no_consumption_and_no_stale_accounts() {
        // No borrows, entry is freshly initialised (last_refresh_secs = now).
        // send_batch with a large threshold should produce no server call.
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock);
        let id = account_id();

        let _ = svc.initialize_account(id).await.unwrap();
        // Large threshold → not stale; no delta → not active.
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        // Nothing changed — no panic, no server call expected.
    }

    #[test]
    async fn send_batch_treats_storage_only_delta_as_activity() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock.clone());
        let id = account_id();
        let entry = svc.initialize_account(id).await.unwrap();
        entry.record_storage_byte_seconds(AgentMode::Durable, 100);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(
            mock.last_batch_update(id)
                .durable_storage_byte_seconds_delta,
            100
        );
    }

    #[test]
    fn storage_remainders_do_not_cross_agent_modes() {
        let entry = AtomicResourceEntry::new(0, 0, 0, 0, 0);

        entry.record_storage_remainder(AgentMode::Durable, 600_000_000);
        entry.record_storage_remainder(AgentMode::Ephemeral, 600_000_000);

        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);

        entry.record_storage_remainder(AgentMode::Durable, 400_000_000);
        entry.record_storage_remainder(AgentMode::Ephemeral, 400_000_000);

        assert_eq!(entry.durable_byte_seconds_delta(), 1);
        assert_eq!(entry.ephemeral_byte_seconds_delta(), 1);
    }

    #[test]
    async fn send_batch_sends_storage_only_delta_when_limits_are_stale() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();
        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(1000),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));
        let svc = make_grpc(mock.clone());
        let entry = svc.initialize_account(id).await.unwrap();
        entry.last_refresh_secs.store(0, Ordering::Release);
        entry.record_storage_byte_seconds(AgentMode::Durable, 100);

        svc.send_batch(STALE_THRESHOLD_SECS).await;

        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(
            mock.last_batch_update(id)
                .durable_storage_byte_seconds_delta,
            100
        );
    }

    #[test]
    async fn send_batch_captures_active_delta_and_zeroes_it() {
        // After borrow_fuel(300): delta = +300.
        // send_batch must swap delta to 0 and include the 300 in the batch.
        // We verify that delta is zeroed; in_flight is cleared only if the server
        // returns a limit update for the account.
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        // Server returns updated limits for the account so in_flight is also cleared.
        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(700),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock.clone());
        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(300);
        entry.record_storage_byte_seconds(AgentMode::Durable, 100);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 200);
        entry.record_memory_gb_seconds(AgentMode::Durable, 3);
        entry.record_memory_gb_seconds(AgentMode::Ephemeral, 4);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 0);
        assert_eq!(
            entry
                .in_flight_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            0
        );
        let update = mock.last_batch_update(id);
        assert_eq!(update.memory_gb_seconds_delta, 7);
        assert_eq!(update.durable_storage_byte_seconds_delta, 100);
        assert_eq!(update.ephemeral_storage_byte_seconds_delta, 200);
        assert_eq!(
            crate::metrics::resources::memory_gb_seconds_total(&id.to_string(), AgentMode::Durable,),
            3.0
        );
        assert_eq!(
            crate::metrics::resources::memory_gb_seconds_total(
                &id.to_string(),
                AgentMode::Ephemeral,
            ),
            4.0
        );
    }

    #[test]
    async fn registry_policy_revision_is_used_by_the_next_batch_without_changing_mode_revision() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();
        let mut current_limits = mock.get_limits_result.lock().unwrap().clone().unwrap();
        current_limits.monthly_policy_revision = 1;
        let mut updated = HashMap::new();
        updated.insert(id, current_limits);
        mock.set_batch_update_response(AccountResourceLimits(updated));
        let svc = make_grpc(mock.clone());
        let entry = svc.initialize_account(id).await.unwrap();

        assert!(entry.borrow_fuel(100));
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
        assert_eq!(mock.last_batch_update(id).monthly_usage_mode_revision, 0);
        assert_eq!(mock.last_batch_update(id).monthly_policy_revision, 0);

        assert!(entry.borrow_fuel(100));
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
        assert_eq!(mock.last_batch_update(id).monthly_usage_mode_revision, 0);
        assert_eq!(mock.last_batch_update(id).monthly_policy_revision, 1);
    }

    #[test]
    async fn send_batch_success_refreshes_fuel_and_clears_in_flight() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(600),
                max_memory_per_worker: 1024,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(400);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.effective_fuel(), 600);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.max_memory.load(Ordering::Acquire), 1024);
    }

    #[test]
    async fn send_batch_success_effective_fuel_reflects_server_value() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(700),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(200);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.effective_fuel(), 700);
    }

    #[test]
    async fn send_batch_failure_clears_in_flight_without_updating_fuel() {
        // On failure: in_flight_delta is zeroed; fuel stays at the old value.
        // The consumed fuel for this interval is lost (not retried).
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        mock.set_batch_update_error();
        let svc = make_grpc(mock);
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(300);
        entry.record_storage_byte_seconds(AgentMode::Durable, 100);
        entry.record_storage_byte_seconds(AgentMode::Ephemeral, 200);
        entry.record_memory_gb_seconds(AgentMode::Durable, 5);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);
        assert_eq!(
            entry
                .in_flight_memory_gb_seconds_delta
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(entry.effective_fuel(), 700);
        assert_eq!(
            crate::metrics::resources::memory_gb_seconds_total(&id.to_string(), AgentMode::Durable,),
            0.0
        );
    }

    #[test]
    async fn send_batch_failure_retires_remainder_only_memory_delivery() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        mock.set_batch_update_error();
        let svc = make_grpc(mock);
        let id = account_id();
        let entry = svc.initialize_account(id).await.unwrap();
        let remainder = BYTE_NANOSECONDS_PER_GB_SECOND / 2;

        entry.record_memory_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: 0,
                remainder,
            },
        );
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        let revision_state = entry.usage_revision_state.lock().unwrap();
        let gate = revision_state.monthly_policy.as_ref().unwrap();
        assert!(gate.in_flight_usage.is_empty());
    }

    #[test]
    async fn send_batch_failure_does_not_double_count_on_next_cycle() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        mock.set_batch_update_error();
        let svc = make_grpc(mock.clone());
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(300);
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await; // fails; 300 is lost

        // New borrows in the second interval
        entry.borrow_fuel(200);
        // delta must only contain the 200, not 300 + 200
        assert_eq!(entry.delta.load(Ordering::Acquire), 200);
    }

    #[test]
    async fn connectivity_outage_keeps_fuel_non_zero_and_allows_borrowing() {
        let mock = Arc::new(MockRegistryService::new(500, 512));
        mock.set_batch_update_error();
        let svc = make_grpc(mock);
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();

        for _ in 0..3 {
            entry.borrow_fuel(100);
            svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
        }

        assert_eq!(entry.effective_fuel(), 200);
        assert!(entry.borrow_fuel(1));
    }

    #[test]
    async fn in_flight_not_double_counted_after_successful_cycle() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(700),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(300);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert!(entry.borrow_fuel(700));
    }

    #[test]
    async fn last_refresh_secs_is_updated_on_successful_batch() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(800),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.last_refresh_secs.store(0, Ordering::Release);

        let before = Utc::now().timestamp();
        entry.borrow_fuel(200);
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
        let after = Utc::now().timestamp();

        let stored = entry.last_refresh_secs.load(Ordering::Acquire);
        assert!(
            stored >= before,
            "last_refresh_secs should be updated on success"
        );
        assert!(stored <= after);
    }

    #[test]
    async fn last_refresh_secs_is_not_updated_on_failed_batch() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        mock.set_batch_update_error();
        let svc = make_grpc(mock);
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();
        let old_ts = 0i64;
        entry.last_refresh_secs.store(old_ts, Ordering::Release);

        entry.borrow_fuel(200);
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.last_refresh_secs.load(Ordering::Acquire), old_ts);
    }

    #[test]
    async fn send_batch_active_account_not_included_in_idle_refresh() {
        // An account with non-zero delta is active — even if stale, it is sent
        // with its real delta (not zero) and must not be double-counted.
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(900),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.last_refresh_secs.store(0, Ordering::Release); // stale
        entry.borrow_fuel(100); // also active

        // With threshold=0 every account is stale, but active accounts take
        // precedence with their real delta.
        svc.send_batch(0).await;

        // Server returned 900 — entry must reflect that, not be zeroed.
        assert_eq!(entry.effective_fuel(), 900);
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    }

    #[test]
    async fn send_batch_idle_stale_account_is_refreshed() {
        // An idle account (delta=0) that is stale must have its limits refreshed
        // via a zero-delta update in the same batch.
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(5000),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.last_refresh_secs.store(0, Ordering::Release); // stale, no borrows

        let before = Utc::now().timestamp();
        svc.send_batch(STALE_THRESHOLD_SECS).await;
        let after = Utc::now().timestamp();

        assert_eq!(entry.effective_fuel(), 5000);
        let stored = entry.last_refresh_secs.load(Ordering::Acquire);
        assert!(stored >= before);
        assert!(stored <= after);
    }

    #[test]
    async fn send_batch_recently_refreshed_idle_account_is_skipped() {
        // An idle account whose last_refresh_secs is recent must not be included.
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let svc = make_grpc(mock);
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();
        // last_refresh_secs is already set to now by new()

        // Large threshold → not stale, no delta → send_batch does nothing.
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        // fuel unchanged (no server call)
        assert_eq!(entry.effective_fuel(), 1000);
    }

    #[test]
    async fn send_batch_idle_failure_does_not_update_last_refresh() {
        // On batch failure, stale idle accounts must retain old last_refresh_secs
        // so they are retried on the next tick.
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        mock.set_batch_update_error();
        let svc = make_grpc(mock);
        let id = account_id();

        let entry = svc.initialize_account(id).await.unwrap();
        let old_ts = 0i64;
        entry.last_refresh_secs.store(old_ts, Ordering::Release);

        svc.send_batch(STALE_THRESHOLD_SECS).await;

        assert_eq!(entry.last_refresh_secs.load(Ordering::Acquire), old_ts);
        assert_eq!(entry.effective_fuel(), 1000);
    }

    #[test]
    async fn idle_account_is_refreshed_when_stale() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(5000),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();

        entry.last_refresh_secs.store(0, Ordering::Release);

        svc.send_batch(STALE_THRESHOLD_SECS).await;

        // Fuel should now reflect the server-returned value
        assert_eq!(entry.effective_fuel(), 5000);
    }

    // -------------------------------------------------------------------------
    // ResourceLimitsGrpc — concurrent agent limit propagation
    // -------------------------------------------------------------------------

    fn mock_with_concurrent_agent_limit(limit: u64) -> Arc<MockRegistryService> {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        *mock.get_limits_result.lock().unwrap() = Ok(ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            max_concurrent_agents_per_executor: limit,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        });
        mock
    }

    #[test]
    async fn initialize_account_propagates_concurrent_agent_limit() {
        let mock = mock_with_concurrent_agent_limit(5);
        let svc = make_grpc(mock);

        let entry = svc.initialize_account(account_id()).await.unwrap();

        assert_eq!(entry.max_concurrent_agents_per_executor(), 5);
    }

    #[test]
    async fn initialize_account_propagates_unlimited_sentinel() {
        // The DB/registry stores 10^18 as "unlimited". The executor stores it
        // as-is in AtomicResourceEntry. The semaphore detects it via >= threshold.
        let mock =
            mock_with_concurrent_agent_limit(AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS);
        let svc = make_grpc(mock);

        let entry = svc.initialize_account(account_id()).await.unwrap();

        assert_eq!(
            entry.max_concurrent_agents_per_executor(),
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS
        );
    }

    #[test]
    async fn update_last_known_limits_refreshes_concurrent_agent_limit() {
        let mock = mock_with_concurrent_agent_limit(5);
        let id = account_id();

        // Batch response returns a raised limit of 10.
        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(900),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: 10,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        assert_eq!(entry.max_concurrent_agents_per_executor(), 5);

        entry.borrow_fuel(100); // trigger active batch
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        // After the batch sync the limit should be updated to 10.
        assert_eq!(entry.max_concurrent_agents_per_executor(), 10);
    }

    #[test]
    async fn update_last_known_limits_reflects_lowered_concurrent_agent_limit() {
        let mock = mock_with_concurrent_agent_limit(10);
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                monthly_policy: monthly_policy(900),
                max_memory_per_worker: 512,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                max_concurrent_agents_per_executor: 3,
                oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        assert_eq!(entry.max_concurrent_agents_per_executor(), 10);

        entry.borrow_fuel(100);
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.max_concurrent_agents_per_executor(), 3);
    }

    #[test]
    async fn disabled_returns_unlimited_concurrent_agent_sentinel() {
        // ResourceLimitsDisabled returns the sentinel value (not u64::MAX directly)
        // matching the convention used throughout the registry service.
        let svc = ResourceLimitsDisabled;
        let entry = svc.initialize_account(account_id()).await.unwrap();
        assert_eq!(
            entry.max_concurrent_agents_per_executor(),
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS
        );
    }

    // -------------------------------------------------------------------------
    // ResourceLimitsDisabled
    // -------------------------------------------------------------------------

    #[test]
    async fn disabled_returns_max_fuel() {
        let svc = ResourceLimitsDisabled;
        let entry = svc.initialize_account(account_id()).await.unwrap();
        assert_eq!(entry.effective_fuel(), u64::MAX);
    }

    #[test]
    async fn disabled_returns_max_memory() {
        let svc = ResourceLimitsDisabled;
        let entry = svc.initialize_account(account_id()).await.unwrap();
        assert_eq!(entry.max_memory_limit(), usize::MAX);
    }

    #[test]
    async fn disabled_borrow_always_succeeds() {
        let svc = ResourceLimitsDisabled;
        let entry = svc.initialize_account(account_id()).await.unwrap();
        assert!(entry.borrow_fuel(u64::MAX / 2));
        // Can borrow again — no real limit
        assert!(entry.borrow_fuel(u64::MAX / 2));
    }
}
