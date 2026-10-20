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
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::{Instrument, error, info_span};

#[derive(Debug)]
pub struct AtomicResourceEntry {
    metering: ResourceUsageMeteringConfig,
    usage_revision_state: Mutex<UsageRevisionState>,
    capacity_updates: tokio::sync::watch::Sender<u64>,
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
    filesystem_limit_updates: OnceLock<tokio::sync::watch::Sender<u64>>,
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

    blob_storage_quota: Mutex<BlobStorageQuota>,

    // Maximum number of concurrently running agents on a single executor for this
    // account. Uses the unlimited sentinel (10^18) when unlimited.
    // Refreshed via update_last_known_limits when batch sync responses arrive.
    max_concurrent_agents_per_executor: AtomicU64,

    // Plan-level per-agent oplog write rate limit (writes per second).
    // UNLIMITED_OPLOG_WRITES_PER_SECOND (10^18) means no rate limiting.
    // Refreshed via update_last_known_limits when batch sync responses arrive.
    oplog_writes_per_second: AtomicU64,
}

/// Publishes resource usage accumulated by an owner-scoped producer.
pub trait ResourceUsageFlusher: Send + Sync + std::fmt::Debug {
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
        self.update.blob_storage_bytes_delta = 0;
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
    policy_status: golem_service_base::model::MonthlyResourcePolicyStatus,
    minimum_policy_revision: u64,
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
    // Diagnostic totals. Full overshoot is already included in the usage sent to Registry.
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
pub struct MonthlyCapacity {
    pub policy_status: golem_service_base::model::MonthlyResourcePolicyStatus,
    pub exhaustion: Option<MonthlyResourceExhaustion>,
    pub policy_revision: u64,
    pub period: AccountUsagePeriod,
    pub fuel_generation: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonthlyResourceExhaustion {
    Compute,
    Memory,
    DurableStorage,
    EphemeralStorage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MonthlyResourceCapacityError {
    Exhausted(MonthlyResourceExhaustion),
    Unavailable,
}

impl From<MonthlyResourceExhaustion> for MonthlyResourceCapacityError {
    fn from(value: MonthlyResourceExhaustion) -> Self {
        Self::Exhausted(value)
    }
}

impl MonthlyResourceCapacityError {
    pub(crate) const UNAVAILABLE_REASON: &'static str =
        "Monthly enforcement unavailable: Registry resource policy could not be refreshed";

    pub(crate) fn unavailable_error() -> WorkerExecutorError {
        WorkerExecutorError::runtime(Self::UNAVAILABLE_REASON)
    }
}

impl MonthlyResourceExhaustion {
    pub const fn reason(self) -> &'static str {
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
struct BlobStorageQuota {
    available_from_server: u64,
    unsynced_delta: i64,
    syncing_delta: i64,
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
        let within_capacity_byte_nanoseconds = maximum_byte_nanoseconds
            .map_or(requested_byte_nanoseconds, |maximum| {
                requested_byte_nanoseconds.min(maximum)
            });
        let measured_units = requested_byte_nanoseconds / BYTE_NANOSECONDS_PER_GB_SECOND;
        let measured_remainder = requested_byte_nanoseconds % BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder = memory.remainder.saturating_add(measured_remainder);
        let remainder_units = memory.remainder / BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder %= BYTE_NANOSECONDS_PER_GB_SECOND;
        let pending = match mode {
            AgentMode::Durable => &mut memory.durable_memory_gb_seconds,
            AgentMode::Ephemeral => &mut memory.ephemeral_memory_gb_seconds,
        };
        *pending = pending.saturating_add(measured_units.saturating_add(remainder_units));
        requested_byte_nanoseconds.saturating_sub(within_capacity_byte_nanoseconds)
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
        let within_capacity_byte_nanoseconds = maximum_byte_nanoseconds
            .map_or(requested_byte_nanoseconds, |maximum| {
                requested_byte_nanoseconds.min(maximum)
            });
        let measured_units = requested_byte_nanoseconds / 1_000_000_000;
        let measured_remainder = requested_byte_nanoseconds % 1_000_000_000;
        let remainder = match mode {
            AgentMode::Durable => &mut storage.durable_storage_remainder,
            AgentMode::Ephemeral => &mut storage.ephemeral_storage_remainder,
        };
        *remainder = remainder.saturating_add(measured_remainder);
        let remainder_units = *remainder / 1_000_000_000;
        *remainder %= 1_000_000_000;
        self.add_storage(mode, measured_units.saturating_add(remainder_units));
        requested_byte_nanoseconds.saturating_sub(within_capacity_byte_nanoseconds)
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
    /// is safe for SQLite REAL, consistent with the Registry's
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

    #[doc(hidden)]
    pub fn new_with_blob_storage_limit(available_blob_storage_bytes: u64) -> Self {
        let mut entry = Self::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            Self::UNLIMITED_CONCURRENT_AGENTS,
        );
        entry
            .blob_storage_quota
            .get_mut()
            .expect("new blob storage quota mutex is not poisoned")
            .available_from_server = available_blob_storage_bytes;
        entry
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
                        policy_status:
                            golem_service_base::model::MonthlyResourcePolicyStatus::Available,
                        minimum_policy_revision: monthly_policy_revision,
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
            capacity_updates: tokio::sync::watch::channel(0).0,
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
            filesystem_limit_updates: OnceLock::new(),
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
            blob_storage_quota: Mutex::new(BlobStorageQuota {
                available_from_server: u64::MAX,
                unsynced_delta: 0,
                syncing_delta: 0,
            }),
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

    pub(crate) fn monthly_memory_check_after(&self, allocated_bytes: u64) -> Option<Duration> {
        if allocated_bytes == 0 || !self.metering.memory {
            return None;
        }
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut revision_state, AccountUsagePeriod::current());
        let remaining = self.memory_settlement_limit(&revision_state)?;
        if remaining == 0 {
            return None;
        }
        let nanoseconds = remaining.div_ceil(allocated_bytes as u128);
        Some(Duration::from_nanos(
            nanoseconds.min(u64::MAX as u128) as u64
        ))
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
            .expect("memory overshoot was recorded without an active monthly policy");
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
            .expect("storage overshoot was recorded without an active monthly policy");
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
                .try_update(Ordering::AcqRel, Ordering::Acquire, |d| {
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

    pub(crate) fn monthly_metering_enabled(&self) -> bool {
        self.metering.compute || self.metering.memory || self.metering.filesystem
    }

    pub fn subscribe_capacity_updates(&self) -> tokio::sync::watch::Receiver<u64> {
        self.capacity_updates.subscribe()
    }

    pub fn with_monthly_capacity<R>(
        &self,
        agent_mode: AgentMode,
        fuel_generation: Option<u64>,
        read: impl FnOnce(MonthlyCapacity) -> R,
    ) -> R {
        let mut state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut state, AccountUsagePeriod::current());
        read(self.monthly_capacity_snapshot(&state, agent_mode, fuel_generation))
    }

    fn monthly_capacity_snapshot(
        &self,
        state: &UsageRevisionState,
        agent_mode: AgentMode,
        fuel_generation: Option<u64>,
    ) -> MonthlyCapacity {
        let policy_status = state.monthly_policy.as_ref().map_or(
            golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            |gate| gate.policy_status,
        );
        let exhaustion = state.monthly_policy.as_ref().and_then(|gate| {
            if gate.policy_status
                != golem_service_base::model::MonthlyResourcePolicyStatus::Available
                || gate.mode != MonthlyUsageMode::HardLimit
                || gate.period != state.current_period
            {
                return None;
            }
            if gate.available_fuel.is_some()
                && self.effective_fuel_with_revision_state(state) == 0
                && fuel_generation.is_none_or(|generation| gate.settled_generation > generation)
            {
                Some(MonthlyResourceExhaustion::Compute)
            } else {
                self.monthly_memory_and_storage_capacity_with_revision_state(state, agent_mode)
                    .err()
            }
        });
        MonthlyCapacity {
            policy_status,
            exhaustion,
            policy_revision: state.current_policy_revision,
            period: state.current_period,
            fuel_generation,
        }
    }

    pub(crate) fn ensure_monthly_policy_available(
        &self,
    ) -> Result<(), MonthlyResourceCapacityError> {
        let state = self.usage_revision_state.lock().unwrap();
        if state.monthly_policy.as_ref().is_some_and(|gate| {
            gate.policy_status
                == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        }) {
            Err(MonthlyResourceCapacityError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(crate) fn monthly_resource_capacity(
        &self,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceCapacityError> {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        let Some(gate) = revision_state.monthly_policy.as_ref() else {
            return Ok(());
        };
        if gate.policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        {
            return Err(MonthlyResourceCapacityError::Unavailable);
        }
        if gate.mode == MonthlyUsageMode::AllowOverage {
            return Ok(());
        }
        if gate.available_fuel.is_some()
            && self.effective_fuel_with_revision_state(&revision_state) == 0
        {
            return Err(MonthlyResourceExhaustion::Compute.into());
        }
        self.monthly_memory_and_storage_capacity_with_revision_state(&revision_state, agent_mode)
            .map_err(Into::into)
    }

    pub(crate) fn monthly_memory_and_storage_capacity(
        &self,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceCapacityError> {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        let Some(gate) = revision_state.monthly_policy.as_ref() else {
            return Ok(());
        };
        if gate.policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        {
            return Err(MonthlyResourceCapacityError::Unavailable);
        }
        if gate.mode == MonthlyUsageMode::AllowOverage {
            return Ok(());
        }
        self.monthly_memory_and_storage_capacity_with_revision_state(&revision_state, agent_mode)
            .map_err(Into::into)
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
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(fuel_delta))
                })
                .ok();
        } else {
            let monthly_usage_mode_revision = revision_state.current_mode_revision;
            revision_state.pending.push_back(CapturedUsageUpdate {
                update: ResourceUsageUpdate {
                    blob_storage_bytes_delta: 0,
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
        self.apply_agent_filesystem_limit_to_targets(allocated_bytes)
            .await
    }

    // Registry refresh publishes desired limits without waiting for an executing invocation to
    // acknowledge its filesystem command. One consumer retains only the latest pending value.
    fn queue_agent_filesystem_limit(self: &Arc<Self>, account_id: AccountId, allocated_bytes: u64) {
        self.max_disk_space
            .store(allocated_bytes, Ordering::Release);
        self.filesystem_limit_updates
            .get_or_init(|| {
                let (sender, mut updates) = tokio::sync::watch::channel(allocated_bytes);
                let entry = Arc::downgrade(self);
                tokio::spawn(async move {
                    while updates.changed().await.is_ok() {
                        let Some(entry) = entry.upgrade() else {
                            break;
                        };
                        let _update = entry.filesystem_limit_update.lock().await;
                        let allocated_bytes = *updates.borrow_and_update();
                        if let Err((owned_agent_id, error)) = entry
                            .apply_agent_filesystem_limit_to_targets(allocated_bytes)
                            .await
                        {
                            error!(
                                account_id = %account_id,
                                agent_id = %owned_agent_id,
                                limit = allocated_bytes,
                                error = %error,
                                "Failed to apply managed agent filesystem limit"
                            );
                        }
                    }
                });
                sender
            })
            .send_replace(allocated_bytes);
    }

    async fn apply_agent_filesystem_limit_to_targets(
        &self,
        allocated_bytes: u64,
    ) -> Result<(), (OwnedAgentId, WorkerExecutorError)> {
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
                blob_storage_bytes_delta: 0,
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
            || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0
            || self.blob_storage_quota.lock().unwrap().unsynced_delta != 0;
        let stale = self.secs_since_last_refresh() >= refresh_threshold_secs;

        let unavailable = revision_state.monthly_policy.as_ref().is_some_and(|gate| {
            gate.policy_status
                == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        });
        if !active && !stale && !unavailable {
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
        captured.update.blob_storage_bytes_delta = {
            let mut quota = self.blob_storage_quota.lock().unwrap();
            let delta = quota.unsynced_delta;
            quota.unsynced_delta = 0;
            quota.syncing_delta = quota.syncing_delta.saturating_add(delta);
            delta
        };
        captured
    }

    fn mark_in_flight(
        &self,
        captured: &CapturedUsageUpdate,
        revision_state: &mut UsageRevisionState,
    ) {
        if captured.update.http_call_count_delta > 0 {
            self.syncing_http_calls
                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(captured.update.http_call_count_delta))
                })
                .ok();
        }
        if captured.update.rpc_call_count_delta > 0 {
            self.syncing_rpc_calls
                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(captured.update.rpc_call_count_delta))
                })
                .ok();
        }
        if captured.update.fuel_delta != 0 {
            self.in_flight_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.update.fuel_delta))
                })
                .ok();
        }
        if captured.update.memory_gb_seconds_delta != 0 {
            self.in_flight_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.update.memory_gb_seconds_delta))
                })
                .ok();
        }
        if captured.durable_memory_gb_seconds_delta != 0 {
            self.in_flight_durable_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.durable_memory_gb_seconds_delta))
                })
                .ok();
        }
        if captured.ephemeral_memory_gb_seconds_delta != 0 {
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
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
                || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0
                || self.blob_storage_quota.lock().unwrap().unsynced_delta != 0;
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
            || captured.update.blob_storage_bytes_delta != 0
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
                || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0
                || self.blob_storage_quota.lock().unwrap().unsynced_delta != 0;
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
            || captured.update.blob_storage_bytes_delta != 0
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
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(in_flight.saturating_sub(delivered.usage.fuel as i64))
                })
                .ok();
            self.in_flight_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight.saturating_sub(delivered.captured.update.memory_gb_seconds_delta),
                    )
                })
                .ok();
            self.in_flight_durable_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight
                            .saturating_sub(delivered.captured.durable_memory_gb_seconds_delta),
                    )
                })
                .ok();
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
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
            golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied,
        )
    }

    fn apply_monthly_snapshot_with_policy_revision(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        policy_status: golem_service_base::model::MonthlyResourcePolicyStatus,
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
        if monthly_policy_revision < current_policy_revision
            || monthly_policy_revision < gate.minimum_policy_revision
        {
            let retry =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied);
            gate.settled_generation = generation;
            if let Some(captured) = retry {
                revision_state.pending.push_front(captured);
            }
            return false;
        }

        if policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable {
            let retry =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied);
            gate.policy_status = policy_status;
            gate.minimum_policy_revision = monthly_policy_revision;
            gate.settled_generation = generation;
            if let Some(captured) = retry {
                revision_state.pending.push_front(captured);
            }
            drop(revision_state);
            self.capacity_updates
                .send_modify(|version| *version = version.wrapping_add(1));
            return true;
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
        gate.policy_status = policy_status;
        gate.minimum_policy_revision = monthly_policy_revision;
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
        drop(revision_state);
        self.capacity_updates
            .send_modify(|version| *version = version.wrapping_add(1));
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
            blob_storage_bytes_delta: 0,
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
    pub(crate) fn capture_pending_usage_for_test(&self) -> Option<ResourceUsageUpdate> {
        self.capture_usage_update(i64::MAX)
            .map(|captured| captured.update)
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
            .try_update(Ordering::AcqRel, Ordering::Acquire, |uhc| {
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
            .try_update(Ordering::AcqRel, Ordering::Acquire, |urc| {
                Some(urc.saturating_add(1))
            })
            .ok();
        true
    }

    pub fn try_record_blob_storage_delta(&self, delta: i64) -> bool {
        let mut quota = self.blob_storage_quota.lock().unwrap();
        if delta <= 0 {
            quota.unsynced_delta = quota.unsynced_delta.saturating_add(delta);
            return true;
        }

        let pending = quota.unsynced_delta.saturating_add(quota.syncing_delta);
        let available = if pending >= 0 {
            quota.available_from_server.saturating_sub(pending as u64)
        } else {
            quota
                .available_from_server
                .saturating_add(pending.unsigned_abs())
        };
        if available < delta as u64 {
            return false;
        }
        quota.unsynced_delta = quota.unsynced_delta.saturating_add(delta);
        true
    }

    pub fn rollback_blob_storage_delta(&self, delta: i64) {
        let mut quota = self.blob_storage_quota.lock().unwrap();
        quota.unsynced_delta = quota.unsynced_delta.saturating_sub(delta);
    }

    #[cfg(test)]
    pub(crate) fn set_available_blob_storage_bytes(&self, value: u64) {
        self.blob_storage_quota
            .lock()
            .unwrap()
            .available_from_server = value;
    }

    #[cfg(test)]
    pub(crate) fn blob_storage_delta_for_test(&self) -> i64 {
        let quota = self.blob_storage_quota.lock().unwrap();
        quota.unsynced_delta.saturating_add(quota.syncing_delta)
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

fn idle_refresh_threshold_secs(
    batch_update_interval: Duration,
    limit_refresh_interval: Duration,
) -> i64 {
    limit_refresh_interval
        .saturating_sub(batch_update_interval)
        .as_secs()
        .min(i64::MAX as u64) as i64
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
            let refresh_threshold_secs =
                idle_refresh_threshold_secs(batch_update_interval, limit_refresh_interval);
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
            .get_resource_limits(
                account_id,
                golem_service_base::clients::registry::ResourceUsageMetering {
                    compute: self.metering.compute,
                    memory: self.metering.memory,
                    filesystem: self.metering.filesystem,
                },
            )
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
                    fuel_accounts = updates.values().filter(|u| u.fuel_delta != 0).count(),
                    memory_accounts = updates
                        .values()
                        .filter(|u| u.memory_gb_seconds_delta != 0)
                        .count(),
                    durable_storage_accounts = updates
                        .values()
                        .filter(|u| u.durable_storage_byte_seconds_delta != 0)
                        .count(),
                    ephemeral_storage_accounts = updates
                        .values()
                        .filter(|u| u.ephemeral_storage_byte_seconds_delta != 0)
                        .count(),
                    http_accounts = updates
                        .values()
                        .filter(|u| u.http_call_count_delta > 0)
                        .count(),
                    rpc_accounts = updates
                        .values()
                        .filter(|u| u.rpc_call_count_delta > 0)
                        .count(),
                    blob_storage_accounts = updates
                        .values()
                        .filter(|u| u.blob_storage_bytes_delta != 0)
                        .count(),
                    stale_idle_accounts = updates
                        .values()
                        .filter(|u| {
                            u.fuel_delta == 0
                                && u.memory_gb_seconds_delta == 0
                                && u.durable_storage_byte_seconds_delta == 0
                                && u.ephemeral_storage_byte_seconds_delta == 0
                                && u.http_call_count_delta == 0
                                && u.rpc_call_count_delta == 0
                                && u.blob_storage_bytes_delta == 0
                        })
                        .count(),
                    "Sending resource usage batch"
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
                                    %account_id,
                                    "Registry did not apply resource usage update; retaining retryable byte-time usage"
                                );
                                self.finish_in_flight_update(
                                    *account_id,
                                    refresh_generation,
                                    AtomicResourceEntry::finish_confirmed_non_application,
                                )
                                .await;
                                self.settle_blob_storage_delta(*account_id, update.blob_storage_bytes_delta).await;
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
                            // Blob stock deltas are not safe to resend after an ambiguous batch
                            // outcome: the registry may already have committed them. Settle every
                            // submitted delta locally and rely on physical reconciliation to repair
                            // either direction of drift.
                            let settled_blob_storage_delta = updates
                                .get(&account_id)
                                .map_or(0, |update| update.blob_storage_bytes_delta);
                            self.update_last_known_limits(
                                account_id,
                                resource_limits,
                                refresh_generation,
                                settled_blob_storage_delta,
                            )
                            .await;
                        }
                    }
                    Err(err) => {
                        record_resource_usage_batch_update_failure();
                        error!(error = %err, "Failed to send batched resource usage updates");
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
                                || update.blob_storage_bytes_delta != 0
                            {
                                error!(
                                    %account_id,
                                    fuel_delta = update.fuel_delta,
                                    memory_gb_seconds_delta = update.memory_gb_seconds_delta,
                                    durable_storage_byte_seconds_delta = update.durable_storage_byte_seconds_delta,
                                    ephemeral_storage_byte_seconds_delta = update.ephemeral_storage_byte_seconds_delta,
                                    http_call_count_delta = update.http_call_count_delta,
                                    rpc_call_count_delta = update.rpc_call_count_delta,
                                    blob_storage_bytes_delta = update.blob_storage_bytes_delta,
                                    "Dropping ambiguous resource usage update"
                                );
                                self.settle_blob_storage_delta(
                                    *account_id,
                                    update.blob_storage_bytes_delta,
                                )
                                .await;
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
        settled_blob_storage_delta: i64,
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if !entry.apply_monthly_snapshot_with_policy_revision(
                refresh_generation,
                updated_limits.monthly_policy.clone(),
                updated_limits.monthly_usage_mode_revision,
                updated_limits.monthly_policy_revision,
                updated_limits.monthly_policy_status,
                updated_limits.usage_update_applied,
            ) {
                self.settle_blob_storage_delta(account_id, settled_blob_storage_delta)
                    .await;
                return;
            }
            entry.update_memory_limit(updated_limits.max_memory_per_worker);
            entry.max_table_elements.store(
                updated_limits.max_table_elements_per_worker as usize,
                Ordering::Release,
            );
            entry
                .queue_agent_filesystem_limit(account_id, updated_limits.max_disk_space_per_worker);
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
            {
                let mut quota = entry.blob_storage_quota.lock().unwrap();
                quota.syncing_delta = quota
                    .syncing_delta
                    .saturating_sub(settled_blob_storage_delta);
                quota.available_from_server = updated_limits.available_blob_storage_bytes;
            }
            entry.max_concurrent_agents_per_executor.store(
                updated_limits.max_concurrent_agents_per_executor,
                Ordering::Release,
            );
            entry
                .oplog_writes_per_second
                .store(updated_limits.oplog_writes_per_second, Ordering::Release);
            entry
                .last_refresh_secs
                .store(Utc::now().timestamp(), Ordering::Release);
        }
    }

    async fn settle_blob_storage_delta(&self, account_id: AccountId, delta: i64) {
        if delta == 0 {
            return;
        }
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            let mut quota = entry.blob_storage_quota.lock().unwrap();
            quota.syncing_delta = quota.syncing_delta.saturating_sub(delta);
            if delta > 0 {
                quota.available_from_server =
                    quota.available_from_server.saturating_sub(delta as u64);
            } else {
                quota.available_from_server = quota
                    .available_from_server
                    .saturating_add(delta.unsigned_abs());
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
                if (self.metering.compute || self.metering.any_byte_time_enabled())
                    && fetched.monthly_policy_status
                        == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
                {
                    return Err(MonthlyResourceCapacityError::unavailable_error());
                }
                let entry = Arc::new(
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
                );
                entry
                    .blob_storage_quota
                    .lock()
                    .unwrap()
                    .available_from_server = fetched.available_blob_storage_bytes;
                Ok::<Arc<AtomicResourceEntry>, WorkerExecutorError>(entry)
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
pub(crate) mod tests;
