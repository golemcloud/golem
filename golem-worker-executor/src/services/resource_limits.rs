// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use golem_common::model::account::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
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
    // TODO: store a last_update timestamp here and fetch the server side values (by including a 0 update) after a certain interval.
    // The reason is that we want to see plan changes / bucket resets even when no work is being performed for that account on this instance right now.
    // Otherwise the first few attempts to start a worker might fail as we are using outdated values.
}

impl AtomicResourceEntry {
    fn new(fuel: u64, max_memory: usize) -> Self {
        Self {
            fuel: AtomicU64::new(fuel),
            delta: AtomicI64::new(0),
            in_flight_delta: AtomicI64::new(0),
            max_memory: AtomicUsize::new(max_memory),
        }
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
) -> Arc<dyn ResourceLimits> {
    match config {
        ResourceLimitsConfig::Grpc(config) => {
            ResourceLimitsGrpc::new(registry_service, config.batch_update_interval)
        }
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
                loop {
                    tick.tick().await;

                    let svc_arc = match svc_weak.upgrade() {
                        Some(s) => s,
                        None => {
                            // service itself was dropped, we can exit
                            break;
                        }
                    };

                    let updates = svc_arc.take_fuel_updates().await;
                    if !updates.is_empty() {
                        svc_arc.send_batch_updates(updates.clone()).await
                    }
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

    async fn take_fuel_updates(&self) -> HashMap<AccountId, i64> {
        let mut updates = HashMap::new();

        let mut keys = Vec::new();
        self.entries
            .iter_async(|k, _| {
                keys.push(*k);
                true
            })
            .await;

        for k in keys {
            if let Some(cell) = self.entries.read_async(&k, |_, e| e.clone()).await {
                if let Some(entry) = cell.get() {
                    let delta = entry.delta.swap(0, Ordering::AcqRel);
                    if delta != 0 {
                        entry
                            .in_flight_delta
                            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                                Some(d.saturating_add(delta))
                            })
                            .ok();
                        updates.insert(k, delta);
                    }
                }
            }
        }

        updates
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

    async fn send_batch_updates(&self, updates: HashMap<AccountId, i64>) {
        tracing::debug!("Sending batch fuel updates");

        let update_limits_result = self.client.batch_update_fuel_usage(updates.clone()).await;

        match update_limits_result {
            Ok(updated_limits) => {
                for (account_id, resource_limits) in updated_limits.0 {
                    self.update_last_known_limits(account_id, resource_limits)
                        .await;
                }
            }
            Err(err) => {
                error!("Failed to send batched resource usage updates: {}", err);
                error!("Lost fuel updates: {:?}", updates);
                for account_id in updates.keys() {
                    self.reset_in_flight_delta(*account_id).await;
                }
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
        Ok(Arc::new(AtomicResourceEntry::new(u64::MAX, usize::MAX)))
    }
}
