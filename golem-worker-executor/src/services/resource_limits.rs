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
use crate::services::agent_memory_meter::BYTE_NANOSECONDS_PER_GB_SECOND;
use crate::services::byte_time_accumulator::ByteTimeSettlement;
use crate::services::golem_config::{ResourceLimitsConfig, ResourceUsageMeteringConfig};
use async_trait::async_trait;
use chrono::Utc;
use golem_common::SafeDisplay;
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_service_base::clients::registry::{RegistryService, ResourceUsageUpdate};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
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
    // Current (cached) value of the account level fuel limits
    fuel: AtomicU64,
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

struct CapturedUsageUpdate {
    update: ResourceUsageUpdate,
    durable_memory_gb_seconds_delta: i64,
    ephemeral_memory_gb_seconds_delta: i64,
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
}

impl AccountUsageAccumulator {
    fn new(config: ResourceUsageMeteringConfig) -> Self {
        Self {
            memory: config.memory.then(MemoryUsageAccumulator::default),
            storage: config.filesystem.then(StorageUsageAccumulator::default),
        }
    }

    fn add_memory(&mut self, mode: AgentMode, units: u128) {
        let Some(memory) = &mut self.memory else {
            return;
        };
        let pending = match mode {
            AgentMode::Durable => &mut memory.durable_memory_gb_seconds,
            AgentMode::Ephemeral => &mut memory.ephemeral_memory_gb_seconds,
        };
        *pending = pending.saturating_add(units);
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

    fn add_memory_settlement(&mut self, mode: AgentMode, settlement: ByteTimeSettlement) {
        let Some(memory) = &mut self.memory else {
            return;
        };
        memory.remainder = memory.remainder.saturating_add(settlement.remainder);
        let remainder_units = memory.remainder / BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder %= BYTE_NANOSECONDS_PER_GB_SECOND;
        self.add_memory(mode, settlement.units.saturating_add(remainder_units));
    }

    fn add_storage_settlement(&mut self, mode: AgentMode, settlement: ByteTimeSettlement) {
        let Some(storage) = &mut self.storage else {
            return;
        };
        let remainder = match mode {
            AgentMode::Durable => &mut storage.durable_storage_remainder,
            AgentMode::Ephemeral => &mut storage.ephemeral_storage_remainder,
        };
        *remainder = remainder.saturating_add(settlement.remainder);
        let remainder_units = *remainder / 1_000_000_000;
        *remainder %= 1_000_000_000;
        self.add_storage(mode, settlement.units.saturating_add(remainder_units));
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

    fn storage(&self, mode: AgentMode) -> u128 {
        self.storage.as_ref().map_or(0, |storage| match mode {
            AgentMode::Durable => storage.durable_storage_byte_seconds,
            AgentMode::Ephemeral => storage.ephemeral_storage_byte_seconds,
        })
    }

    fn capture(&mut self) -> CapturedAccountUsage {
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
    // XFS supports block sizes up to 64 KiB, so this remains exactly representable.
    pub(crate) const EFFECTIVELY_UNLIMITED_DISK_SPACE: u64 = u64::MAX - u16::MAX as u64;

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
        Self {
            metering,
            fuel: AtomicU64::new(if metering.compute { fuel } else { u64::MAX }),
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

    fn effective_fuel(&self) -> u64 {
        if !self.metering.compute {
            return u64::MAX;
        }
        let fuel = self.fuel.load(Ordering::Acquire);
        let delta = self.delta.load(Ordering::Acquire);
        let in_flight = self.in_flight_delta.load(Ordering::Acquire);

        // compute sum as i128 to avoid overflow
        let sum = fuel as i128 + delta as i128 + in_flight as i128;

        sum.max(0).min(u64::MAX as i128) as u64
    }

    #[cfg(test)]
    pub(crate) fn fuel_delta(&self) -> i64 {
        self.delta.load(Ordering::Acquire)
    }

    pub fn borrow_fuel(&self, amount: u64) -> bool {
        if !self.metering.compute {
            return true;
        }
        let available = self.effective_fuel();

        if amount == 0 {
            return true;
        };

        if amount <= available {
            let amt_i64 = amount.min(i64::MAX as u64) as i64;
            self.delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                    Some(d.saturating_add(amt_i64))
                })
                .ok();
            record_fuel_borrow(amount);
            true
        } else {
            false
        }
    }

    pub fn has_effective_fuel(&self) -> bool {
        self.effective_fuel() > 0
    }

    pub fn return_fuel(&self, amount: u64) {
        if !self.metering.compute {
            return;
        }
        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.delta
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                Some(d.saturating_sub(amt_i64))
            })
            .ok();
        record_fuel_return(amount);
    }

    pub fn record_overdraft_debt(&self, amount: u64) {
        if !self.metering.compute || amount == 0 {
            return;
        }

        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.delta
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                Some(d.saturating_add(amt_i64))
            })
            .ok();
        record_ephemeral_overdraft_fuel(amount);
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
        if self.metering.filesystem
            && amount > 0
            && let Some(accumulator) = &self.account_usage_accumulator
        {
            accumulator
                .lock()
                .unwrap()
                .add_storage(mode, amount as u128);
        }
    }

    pub(crate) fn record_resource_usage(
        &self,
        mode: AgentMode,
        memory_gb_seconds: i64,
        storage_byte_seconds: i64,
    ) {
        let Some(accumulator) = &self.account_usage_accumulator else {
            return;
        };
        let mut accumulator = accumulator.lock().unwrap();
        if self.metering.memory && memory_gb_seconds > 0 {
            accumulator.add_memory(mode, memory_gb_seconds as u128);
        }
        if self.metering.filesystem && storage_byte_seconds > 0 {
            accumulator.add_storage(mode, storage_byte_seconds as u128);
        }
    }

    pub(crate) fn record_resource_settlement(
        &self,
        mode: AgentMode,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) {
        let Some(accumulator) = &self.account_usage_accumulator else {
            return;
        };
        let mut accumulator = accumulator.lock().unwrap();
        if self.metering.memory {
            accumulator.add_memory_settlement(mode, memory);
        }
        if self.metering.filesystem {
            accumulator.add_storage_settlement(mode, storage);
        }
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
        self.flush_active_resource_usage();
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

        let fuel_delta = if self.metering.compute {
            self.delta.swap(0, Ordering::AcqRel)
        } else {
            0
        };
        let captured_usage = self
            .account_usage_accumulator
            .as_ref()
            .map_or_else(CapturedAccountUsage::default, |accumulator| {
                accumulator.lock().unwrap().capture()
            });
        let memory_gb_seconds_delta = captured_usage.memory_gb_seconds;
        let durable_memory_gb_seconds_delta = captured_usage.durable_memory_gb_seconds;
        let ephemeral_memory_gb_seconds_delta = captured_usage.ephemeral_memory_gb_seconds;
        let durable_storage_byte_seconds_delta = captured_usage.durable_storage_byte_seconds;
        let ephemeral_storage_byte_seconds_delta = captured_usage.ephemeral_storage_byte_seconds;
        let http_count = self.unsynced_http_calls.swap(0, Ordering::AcqRel);
        let rpc_count = self.unsynced_rpc_calls.swap(0, Ordering::AcqRel);
        if http_count > 0 {
            self.syncing_http_calls
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(http_count))
                })
                .ok();
        }
        if rpc_count > 0 {
            self.syncing_rpc_calls
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(rpc_count))
                })
                .ok();
        }
        if fuel_delta != 0 {
            self.in_flight_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(fuel_delta))
                })
                .ok();
        }
        if memory_gb_seconds_delta != 0 {
            self.in_flight_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(memory_gb_seconds_delta))
                })
                .ok();
        }
        if durable_memory_gb_seconds_delta != 0 {
            self.in_flight_durable_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(durable_memory_gb_seconds_delta))
                })
                .ok();
        }
        if ephemeral_memory_gb_seconds_delta != 0 {
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(ephemeral_memory_gb_seconds_delta))
                })
                .ok();
        }
        Some(CapturedUsageUpdate {
            update: ResourceUsageUpdate {
                fuel_delta,
                memory_gb_seconds_delta,
                http_call_count_delta: http_count,
                rpc_call_count_delta: rpc_count,
                durable_storage_byte_seconds_delta,
                ephemeral_storage_byte_seconds_delta,
            },
            durable_memory_gb_seconds_delta,
            ephemeral_memory_gb_seconds_delta,
        })
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
        if self.metering.memory
            && amount > 0
            && let Some(accumulator) = &self.account_usage_accumulator
        {
            accumulator.lock().unwrap().add_memory(mode, amount as u128);
        }
    }

    pub(crate) fn record_memory_settlement(&self, mode: AgentMode, settlement: ByteTimeSettlement) {
        if self.metering.memory
            && let Some(accumulator) = &self.account_usage_accumulator
        {
            accumulator
                .lock()
                .unwrap()
                .add_memory_settlement(mode, settlement);
        }
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
        let available = self
            .available_http_calls_from_server
            .load(Ordering::Acquire);
        let unsynced = self.unsynced_http_calls.load(Ordering::Acquire);
        let syncing = self.syncing_http_calls.load(Ordering::Acquire);
        available.saturating_sub(unsynced).saturating_sub(syncing)
    }

    /// Returns the number of RPC calls remaining in this billing period.
    pub fn remaining_rpc_calls(&self) -> u64 {
        let available = self.available_rpc_calls_from_server.load(Ordering::Acquire);
        let unsynced = self.unsynced_rpc_calls.load(Ordering::Acquire);
        let syncing = self.syncing_rpc_calls.load(Ordering::Acquire);
        available.saturating_sub(unsynced).saturating_sub(syncing)
    }

    /// Records one outgoing HTTP call against the monthly account quota.
    ///
    /// Returns `false` when the remaining HTTP call budget is zero,
    /// signalling that the worker should be suspended at the next opportunity.
    pub fn record_http_call(&self) -> bool {
        if self.remaining_http_calls() == 0 {
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
        if self.remaining_rpc_calls() == 0 {
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
                                    "Registry did not apply resource usage update for account {account_id}; dropping the in-flight update"
                                );
                                self.reset_in_flight_delta(*account_id).await;
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
                            self.update_last_known_limits(account_id, resource_limits)
                                .await;
                        }
                    }
                    Err(err) => {
                        record_resource_usage_batch_update_failure();
                        error!("Failed to send batched resource usage updates: {}", err);
                        for (account_id, update) in &updates {
                            if update.fuel_delta != 0
                                || update.memory_gb_seconds_delta != 0
                                || update.durable_storage_byte_seconds_delta != 0
                                || update.ephemeral_storage_byte_seconds_delta != 0
                                || update.http_call_count_delta > 0
                                || update.rpc_call_count_delta > 0
                            {
                                error!(
                                    "Lost resource usage updates for account {account_id}: fuel_delta={}, memory_gb_seconds_delta={}, durable_storage_byte_seconds_delta={}, ephemeral_storage_byte_seconds_delta={}, http_call_count_delta={}, rpc_call_count_delta={}",
                                    update.fuel_delta,
                                    update.memory_gb_seconds_delta,
                                    update.durable_storage_byte_seconds_delta,
                                    update.ephemeral_storage_byte_seconds_delta,
                                    update.http_call_count_delta,
                                    update.rpc_call_count_delta,
                                );
                                self.reset_in_flight_delta(*account_id).await;
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
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if self.metering.compute {
                entry.in_flight_delta.store(0, Ordering::Release);
                entry
                    .fuel
                    .store(updated_limits.available_fuel, Ordering::Release);
            }
            if self.metering.memory {
                entry
                    .in_flight_memory_gb_seconds_delta
                    .store(0, Ordering::Release);
                entry
                    .in_flight_durable_memory_gb_seconds_delta
                    .store(0, Ordering::Release);
                entry
                    .in_flight_ephemeral_memory_gb_seconds_delta
                    .store(0, Ordering::Release);
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

    async fn reset_in_flight_delta(&self, account_id: AccountId) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await
            && let Some(entry) = cell.get()
        {
            if self.metering.compute {
                entry.in_flight_delta.swap(0, Ordering::AcqRel);
            }
            if self.metering.memory {
                entry
                    .in_flight_memory_gb_seconds_delta
                    .swap(0, Ordering::AcqRel);
                entry
                    .in_flight_durable_memory_gb_seconds_delta
                    .swap(0, Ordering::AcqRel);
                entry
                    .in_flight_ephemeral_memory_gb_seconds_delta
                    .swap(0, Ordering::AcqRel);
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
                let fetched = self.fetch_resource_limits(account_id).await?;
                Ok::<Arc<AtomicResourceEntry>, WorkerExecutorError>(Arc::new(
                    AtomicResourceEntry::new_with_all_limits_and_metering(
                        fetched.available_fuel,
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
    use std::sync::Mutex;
    use test_r::test;
    use uuid::Uuid;

    test_r::enable!();

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
        );
        accumulator.add_storage_settlement(
            AgentMode::Durable,
            ByteTimeSettlement {
                units: oversized,
                remainder: 0,
            },
        );

        assert_eq!(
            accumulator.capture(),
            CapturedAccountUsage {
                memory_gb_seconds: i64::MAX,
                durable_memory_gb_seconds: i64::MAX,
                ephemeral_memory_gb_seconds: 0,
                durable_storage_byte_seconds: i64::MAX,
                ephemeral_storage_byte_seconds: 0,
            }
        );
        assert_eq!(
            accumulator.capture(),
            CapturedAccountUsage {
                memory_gb_seconds: 7,
                durable_memory_gb_seconds: 7,
                ephemeral_memory_gb_seconds: 0,
                durable_storage_byte_seconds: 7,
                ephemeral_storage_byte_seconds: 0,
            }
        );
        assert!(!accumulator.is_active());
    }

    // -------------------------------------------------------------------------
    // AtomicResourceEntry
    // -------------------------------------------------------------------------

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
    fn effective_fuel_with_zero_delta() {
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        assert_eq!(entry.effective_fuel(), 1000);
    }

    #[test]
    fn effective_fuel_sums_fuel_delta_and_in_flight() {
        // delta = +200 (fuel lent), in_flight = +50 (earlier batch in transit)
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.delta.store(200, Ordering::Release);
        entry.in_flight_delta.store(50, Ordering::Release);
        assert_eq!(entry.effective_fuel(), 1250);
    }

    #[test]
    fn effective_fuel_clamps_to_zero_when_sum_is_negative() {
        // delta negative (more returned than borrowed): 100 + (-200) = -100 → 0
        let entry = AtomicResourceEntry::new(100, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.delta.store(-200, Ordering::Release);
        assert_eq!(entry.effective_fuel(), 0);
    }

    #[test]
    fn effective_fuel_clamps_to_u64_max_when_sum_overflows() {
        // u64::MAX + i64::MAX overflows u64 in i128 arithmetic → clamped
        let entry = AtomicResourceEntry::new(u64::MAX, 0, usize::MAX, u64::MAX, u64::MAX);
        entry.delta.store(i64::MAX, Ordering::Release);
        assert_eq!(entry.effective_fuel(), u64::MAX);
    }

    #[test]
    fn borrow_fuel_succeeds_and_increases_delta() {
        let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(entry.borrow_fuel(300));
        // borrow_fuel records the loan by adding positively to delta
        assert_eq!(entry.delta.load(Ordering::Acquire), 300);
        // effective_fuel = 1000 + 300 = 1300 (optimistic: more appears available)
        assert_eq!(entry.effective_fuel(), 1300);
    }

    #[test]
    fn borrow_fuel_fails_when_effective_fuel_is_zero() {
        // fuel=0, delta=0 → effective=0; any non-zero borrow fails
        let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(!entry.borrow_fuel(1));
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    }

    #[test]
    fn borrow_fuel_fails_when_amount_exceeds_effective_fuel() {
        // fuel=100, effective=100; borrowing 101 must fail
        let entry = AtomicResourceEntry::new(100, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(!entry.borrow_fuel(101));
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
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
    fn borrow_fuel_one_over_effective_fuel_fails() {
        // Borrowing effective_fuel + 1 must fail
        let entry = AtomicResourceEntry::new(500, 0, usize::MAX, u64::MAX, u64::MAX);
        assert!(!entry.borrow_fuel(501));
        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
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
        assert_eq!(entry.effective_fuel(), 3000);
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
            available_fuel: 1000,
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
                available_fuel: 1000,
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
            available_fuel: 1000,
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
        });
        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                available_fuel: 1000,
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
        last_batch_updates: Mutex<HashMap<AccountId, ResourceUsageUpdate>>,
    }

    impl MockRegistryService {
        fn new(available_fuel: u64, max_memory: u64) -> Self {
            Self {
                get_limits_result: Mutex::new(Ok(ServiceResourceLimits {
                    available_fuel,
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
                })),
                batch_update_result: Mutex::new(Ok(AccountResourceLimits(HashMap::new()))),
                last_batch_updates: Mutex::new(HashMap::new()),
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
            *self.last_batch_updates.lock().unwrap() = updates;
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
        // Pass an already-cancelled token so the background batch task exits
        // immediately in its first select! — before it can call send_batch.
        // Tests drive the batch cycle manually via send_batch for deterministic,
        // race-free control.
        let token = CancellationToken::new();
        token.cancel();
        ResourceLimitsGrpc::new(
            mock,
            Duration::from_secs(3600),
            Duration::from_secs(300),
            ResourceUsageMeteringConfig::all_enabled(),
            token,
        )
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
                available_fuel: 1000,
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
                available_fuel: 700,
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
    async fn send_batch_success_refreshes_fuel_and_clears_in_flight() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                available_fuel: 600,
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
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(400);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.fuel.load(Ordering::Acquire), 600);
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
                available_fuel: 700,
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
        assert_eq!(entry.fuel.load(Ordering::Acquire), 1000);
        assert_eq!(
            crate::metrics::resources::memory_gb_seconds_total(&id.to_string(), AgentMode::Durable,),
            0.0
        );
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

        assert_eq!(entry.fuel.load(Ordering::Acquire), 500);
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
                available_fuel: 700,
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
                available_fuel: 800,
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
                available_fuel: 900,
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
        assert_eq!(entry.fuel.load(Ordering::Acquire), 900);
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
                available_fuel: 5000,
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
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.last_refresh_secs.store(0, Ordering::Release); // stale, no borrows

        let before = Utc::now().timestamp();
        svc.send_batch(STALE_THRESHOLD_SECS).await;
        let after = Utc::now().timestamp();

        assert_eq!(entry.fuel.load(Ordering::Acquire), 5000);
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
        assert_eq!(entry.fuel.load(Ordering::Acquire), 1000);
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
        assert_eq!(entry.fuel.load(Ordering::Acquire), 1000);
    }

    #[test]
    async fn idle_account_is_refreshed_when_stale() {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        let id = account_id();

        let mut updated = HashMap::new();
        updated.insert(
            id,
            ServiceResourceLimits {
                available_fuel: 5000,
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
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();

        entry.last_refresh_secs.store(0, Ordering::Release);

        svc.send_batch(STALE_THRESHOLD_SECS).await;

        // Fuel should now reflect the server-returned value
        assert_eq!(entry.fuel.load(Ordering::Acquire), 5000);
    }

    // -------------------------------------------------------------------------
    // ResourceLimitsGrpc — concurrent agent limit propagation
    // -------------------------------------------------------------------------

    fn mock_with_concurrent_agent_limit(limit: u64) -> Arc<MockRegistryService> {
        let mock = Arc::new(MockRegistryService::new(1000, 512));
        *mock.get_limits_result.lock().unwrap() = Ok(ServiceResourceLimits {
            available_fuel: 1000,
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
                available_fuel: 900,
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
                available_fuel: 900,
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
