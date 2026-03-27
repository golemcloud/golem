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

use crate::metrics::resources::{record_fuel_borrow, record_fuel_return};
use crate::services::golem_config::ResourceLimitsConfig;
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::account::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::{error, span, Instrument, Level};

#[derive(Debug)]
pub struct AtomicResourceEntry {
    // Current (cached) value of the account level fuel limits
    fuel: AtomicU64,
    // any local fuel consumption that was not yet sent to the server
    delta: AtomicI64,
    // any fuel consumption that is currently in flight to the server
    in_flight_delta: AtomicI64,
    // Current (cached) value of the account level worker memory limits
    max_memory: AtomicUsize,
    // Current (cached) value of the account level worker function table element limits
    max_table_elements: AtomicUsize,
    // Current (cached) value of the account level per-worker disk space limit
    max_disk_space: AtomicU64,
    // Unix timestamp (seconds) of the last time fuel/memory were refreshed from
    // the server. Used by the background loop to detect idle accounts whose
    // cached limits have grown stale (e.g. after a plan change or monthly reset).
    last_refresh_secs: AtomicI64,
    // Maximum number of concurrently running agents on a single executor for this
    // account. u64::MAX means unlimited (no semaphore acquisition needed).
    // Refreshed via update_last_known_limits when batch sync responses arrive.
    max_concurrent_agents_per_executor: AtomicU64,
}

impl AtomicResourceEntry {
    /// Sentinel value used in the database and service config to represent
    /// "unlimited" for the concurrent agents per executor limit.
    /// `1_000_000_000_000_000_000` (10^18) — fits in i64 (TOML max) and
    /// is safe for SQLite REAL, consistent with `monthly_gas_limit` and the
    /// `default_unlimited()` convention from PR #3068.
    pub const UNLIMITED_CONCURRENT_AGENTS: u64 = 1_000_000_000_000_000_000;

    pub fn new(
        fuel: u64,
        max_memory: usize,
        max_table_elements: usize,
        max_disk_space: u64,
        max_concurrent_agents_per_executor: u64,
    ) -> Self {
        Self {
            fuel: AtomicU64::new(fuel),
            delta: AtomicI64::new(0),
            in_flight_delta: AtomicI64::new(0),
            max_memory: AtomicUsize::new(max_memory),
            max_table_elements: AtomicUsize::new(max_table_elements),
            max_disk_space: AtomicU64::new(max_disk_space),
            last_refresh_secs: AtomicI64::new(Utc::now().timestamp()),
            max_concurrent_agents_per_executor: AtomicU64::new(max_concurrent_agents_per_executor),
        }
    }

    fn secs_since_last_refresh(&self) -> i64 {
        Utc::now()
            .timestamp()
            .saturating_sub(self.last_refresh_secs.load(Ordering::Acquire))
    }

    fn effective_fuel(&self) -> u64 {
        let fuel = self.fuel.load(Ordering::Acquire);
        let delta = self.delta.load(Ordering::Acquire);
        let in_flight = self.in_flight_delta.load(Ordering::Acquire);

        // compute sum as i128 to avoid overflow
        let sum = fuel as i128 + delta as i128 + in_flight as i128;

        sum.max(0).min(u64::MAX as i128) as u64
    }

    pub fn borrow_fuel(&self, amount: u64) -> bool {
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

    pub fn return_fuel(&self, amount: u64) {
        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.delta
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                Some(d.saturating_sub(amt_i64))
            })
            .ok();
        record_fuel_return(amount);
    }

    pub fn max_memory_limit(&self) -> usize {
        self.max_memory.load(Ordering::Acquire)
    }

    pub fn max_table_elements_limit(&self) -> usize {
        self.max_table_elements.load(Ordering::Acquire)
    }

    pub fn max_disk_space_limit(&self) -> u64 {
        self.max_disk_space.load(Ordering::Acquire)
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
    // Get a handle to the shared resource limits entry for the account. This might be updated in the background
    // as fuel usage is reported to registry service
    async fn initialize_account(
        &self,
        account_id: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError>;
}

pub fn configured(
    config: &ResourceLimitsConfig,
    registry_service: Arc<dyn RegistryService>,
    shutdown_token: CancellationToken,
) -> Arc<dyn ResourceLimits> {
    match config {
        ResourceLimitsConfig::Grpc(config) => ResourceLimitsGrpc::new(
            registry_service,
            config.batch_update_interval,
            config.limit_refresh_interval,
            shutdown_token,
        ),
        ResourceLimitsConfig::Disabled(_) => Arc::new(ResourceLimitsDisabled),
    }
}

// Note:
// this is biased towards allowing borrows when it doubt, but might allow slight overborrowing temporarily.
// Internally we store deltas as i64 for simplicitly. If more fuel is consumed / returned within one update time slice
// than the i64 limits, those updates will be lost.
pub struct ResourceLimitsGrpc {
    client: Arc<dyn RegistryService>,
    entries: scc::HashMap<AccountId, Arc<OnceCell<Arc<AtomicResourceEntry>>>>,
}

impl ResourceLimitsGrpc {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        batch_update_interval: Duration,
        limit_refresh_interval: Duration,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let svc = Self {
            client: registry_service,
            entries: scc::HashMap::new(),
        };
        let svc = Arc::new(svc);
        let svc_weak = Arc::downgrade(&svc);

        // Background task for batch updates
        tokio::spawn(
            async move {
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
            }
            .instrument(span!(parent: None, Level::INFO, "Resource limits batch updates")),
        );

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

    /// Builds and sends a single batch to the registry covering both active
    /// accounts (non-zero fuel delta) and stale idle accounts (zero delta for
    /// a limit refresh). On success, updates all entries via
    /// `update_last_known_limits`. On failure, resets `in_flight_delta` for
    /// active accounts (so their deltas are not double-counted next cycle) and
    /// leaves `last_refresh_secs` unchanged for stale idle accounts (so they
    /// are retried next tick).
    async fn send_batch(&self, refresh_threshold_secs: i64) {
        // Collect active updates (non-zero delta) and move delta → in_flight.
        let mut updates: HashMap<AccountId, i64> = HashMap::new();

        self.entries
            .iter_async(|k, cell| {
                if let Some(entry) = cell.get() {
                    let delta = entry.delta.swap(0, Ordering::AcqRel);
                    if delta != 0 {
                        entry
                            .in_flight_delta
                            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                                Some(d.saturating_add(delta))
                            })
                            .ok();
                        updates.insert(*k, delta);
                    } else if entry.secs_since_last_refresh() >= refresh_threshold_secs {
                        // Stale idle account: include with zero delta to fetch
                        // updated limits without reporting any consumption.
                        updates.insert(*k, 0);
                    }
                }
                true
            })
            .await;

        if updates.is_empty() {
            return;
        }

        tracing::debug!(
            "Sending batch: {} active, {} stale idle account(s)",
            updates.values().filter(|&&d| d != 0).count(),
            updates.values().filter(|&&d| d == 0).count(),
        );

        match self.client.batch_update_fuel_usage(updates.clone()).await {
            Ok(updated_limits) => {
                for (account_id, resource_limits) in updated_limits.0 {
                    self.update_last_known_limits(account_id, resource_limits)
                        .await;
                }
            }
            Err(err) => {
                error!("Failed to send batched resource usage updates: {}", err);
                // Reset in_flight_delta only for active accounts — their deltas
                // were swapped out and must not be double-counted next cycle.
                // Stale idle accounts had delta=0 so nothing to reset; their
                // last_refresh_secs stays unchanged and they will be retried.
                for (account_id, delta) in &updates {
                    if *delta != 0 {
                        error!("Lost fuel updates for account {account_id}: {delta}");
                        self.reset_in_flight_delta(*account_id).await;
                    }
                }
            }
        }
    }

    async fn update_last_known_limits(
        &self,
        account_id: AccountId,
        updated_limits: golem_service_base::model::ResourceLimits,
    ) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await {
            if let Some(entry) = cell.get() {
                entry.in_flight_delta.store(0, Ordering::Release);
                entry
                    .fuel
                    .store(updated_limits.available_fuel, Ordering::Release);
                entry.max_memory.store(
                    updated_limits.max_memory_per_worker as usize,
                    Ordering::Release,
                );
                entry.max_table_elements.store(
                    updated_limits.max_table_elements_per_worker as usize,
                    Ordering::Release,
                );
                entry
                    .max_disk_space
                    .store(updated_limits.max_disk_space_per_worker, Ordering::Release);
                entry.max_concurrent_agents_per_executor.store(
                    updated_limits.max_concurrent_agents_per_executor,
                    Ordering::Release,
                );
                entry
                    .last_refresh_secs
                    .store(Utc::now().timestamp(), Ordering::Release);
            }
        }
    }

    async fn reset_in_flight_delta(&self, account_id: AccountId) {
        if let Some(cell) = self.entries.read_async(&account_id, |_, e| e.clone()).await {
            if let Some(entry) = cell.get() {
                entry.in_flight_delta.swap(0, Ordering::AcqRel);
            }
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
                    AtomicResourceEntry::new(
                        fetched.available_fuel,
                        fetched.max_memory_per_worker as usize,
                        fetched.max_table_elements_per_worker as usize,
                        fetched.max_disk_space_per_worker,
                        fetched.max_concurrent_agents_per_executor,
                    ),
                ))
            })
            .await?;

        Ok(entry.clone())
    }
}

pub struct ResourceLimitsDisabled;

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
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::{AgentTypeName, RegisteredAgentType, ResolvedAgentType};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::TokenSecret;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::resource_definition::{
        ResourceDefinition, ResourceDefinitionId, ResourceName,
    };
    use golem_common::model::AgentId;
    use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
    use golem_service_base::custom_api::CompiledRoutes;
    use golem_service_base::mcp::CompiledMcp;
    use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment};
    use golem_service_base::model::component::Component;
    use golem_service_base::model::environment::EnvironmentState;
    use golem_service_base::model::{
        AccountResourceLimits, ResourceLimits as ServiceResourceLimits,
    };
    use std::sync::Mutex;
    use test_r::test;
    use uuid::Uuid;

    test_r::enable!();

    // -------------------------------------------------------------------------
    // AtomicResourceEntry
    // -------------------------------------------------------------------------

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
    }

    impl MockRegistryService {
        fn new(available_fuel: u64, max_memory: u64) -> Self {
            Self {
                get_limits_result: Mutex::new(Ok(ServiceResourceLimits {
                    available_fuel,
                    max_memory_per_worker: max_memory,
                    max_table_elements_per_worker: u64::MAX,
                    max_disk_space_per_worker: u64::MAX,
                    max_concurrent_agents_per_executor: u64::MAX,
                })),
                batch_update_result: Mutex::new(Ok(AccountResourceLimits(HashMap::new()))),
            }
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
    }

    #[async_trait]
    impl RegistryService for MockRegistryService {
        async fn authenticate_token(
            &self,
            _token: &TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_auth_details_for_environment(
            &self,
            _environment_id: EnvironmentId,
            _include_deleted: bool,
            _auth_ctx: &AuthCtx,
        ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
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

        async fn update_worker_limit(
            &self,
            _account_id: AccountId,
            _agent_id: &AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_connection_limit(
            &self,
            _account_id: AccountId,
            _agent_id: &AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_fuel_usage(
            &self,
            _updates: HashMap<AccountId, i64>,
        ) -> Result<AccountResourceLimits, RegistryServiceError> {
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

        async fn resolve_latest_agent_type_by_names(
            &self,
            _account_id: &AccountId,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
        ) -> Result<RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_at_deployment(
            &self,
            _account_id: &AccountId,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
            _deployment_revision: DeploymentRevision,
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
                max_concurrent_agents_per_executor: u64::MAX,
            },
        );
        mock.set_batch_update_response(AccountResourceLimits(updated));

        let svc = make_grpc(mock);
        let entry = svc.initialize_account(id).await.unwrap();
        entry.borrow_fuel(300);

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
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
                max_concurrent_agents_per_executor: u64::MAX,
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
                max_concurrent_agents_per_executor: u64::MAX,
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

        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

        assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
        assert_eq!(entry.fuel.load(Ordering::Acquire), 1000);
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
                max_concurrent_agents_per_executor: u64::MAX,
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
                max_concurrent_agents_per_executor: u64::MAX,
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
                max_concurrent_agents_per_executor: u64::MAX,
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
                max_concurrent_agents_per_executor: u64::MAX,
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
                max_concurrent_agents_per_executor: u64::MAX,
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
            max_concurrent_agents_per_executor: limit,
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
                max_concurrent_agents_per_executor: 10,
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
                max_concurrent_agents_per_executor: 3,
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
