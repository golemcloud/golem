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

#[cfg(test)]
use refresh::retain_unaccepted_delivery;

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
}

mod capacity;
mod counters;
mod enforcement;
mod fuel;
mod grpc;
mod refresh;
mod usage;

use capacity::{
    monthly_policy_available_memory_byte_nanoseconds,
    monthly_policy_available_storage_byte_nanoseconds,
};
#[cfg(test)]
use grpc::idle_refresh_threshold_secs;
