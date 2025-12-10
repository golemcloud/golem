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
use crate::model::CurrentResourceLimits;
use crate::services::golem_config::ResourceLimitsConfig;
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;
use tracing::{error, span, Instrument, Level};

#[async_trait]
pub trait ResourceLimits: Send + Sync {
    /// Tries to borrow fuel for a worker for the given account. Returns the maximum amount of
    /// fuel the worker can consume at once.
    /// If the worker runs out of fuel, it can try borrowing more fuel with the same function.
    /// If the worker finishes running it can give back the remaining fuel with `return_fuel`.
    async fn borrow_fuel(
        &self,
        account_id: &AccountId,
        amount: u64,
    ) -> Result<u64, WorkerExecutorError>;

    /// Sync version of `borrow_fuel` to be used from the epoch callback.
    /// This only works if the ResourceLimits implementation is already has a cached resource limit
    /// for the given account, but this can be guaranteed by always calling `borrow_fuel` first.
    fn borrow_fuel_sync(&self, account_id: &AccountId, amount: u64) -> Option<u64>;

    /// Returns some unused fuel for a given user
    async fn return_fuel(
        &self,
        account_id: &AccountId,
        remaining: u64,
    ) -> Result<(), WorkerExecutorError>;

    async fn get_max_memory(&self, account_id: &AccountId) -> Result<usize, WorkerExecutorError>;
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

#[derive(Debug, Clone)]
pub struct CurrentResourceLimitsEntry {
    limits: CurrentResourceLimits,
    delta: i128,
}

/// The default ResourceLimits implementation
/// - can query the Cloud Services for information about the account's available resources
/// - caches this information for a given amount of time
/// - periodically sends batched patches to the Cloud Services to update the account's resources
pub struct ResourceLimitsGrpc {
    client: Arc<dyn RegistryService>,
    current_limits: scc::HashMap<AccountId, CurrentResourceLimitsEntry>,
}

impl ResourceLimitsGrpc {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        batch_update_interval: Duration,
    ) -> Arc<Self> {
        let svc = Self {
            client: registry_service,
            current_limits: scc::HashMap::new(),
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
                        None => break,
                    };

                    let updates = svc_arc.take_fuel_updates().await;
                    if !updates.is_empty() {
                        if let Err(err) = svc_arc.send_batch_updates(updates.clone()).await {
                            error!("Failed to send batched resource usage updates: {}", err);
                            error!("Lost fuel updates: {:?}", updates);
                        }
                    }
                }
            }
            .instrument(span!(parent: None, Level::INFO, "Resource limits batch updates")),
        );

        svc
    }

    async fn send_batch_updates(
        &self,
        updates: HashMap<AccountId, i64>,
    ) -> Result<(), WorkerExecutorError> {
        tracing::debug!("Sending batch fuel updates");

        let updated_limits = self
            .client
            .batch_update_fuel_usage(updates, &AuthCtx::System)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed updating fuel usage: {}",
                    e.to_safe_string()
                ))
            })?;

        for (account_id, resource_limits) in &updated_limits.0 {
            self.update_last_known_limits(
                account_id,
                &CurrentResourceLimits {
                    fuel: resource_limits.available_fuel,
                    max_memory: resource_limits.max_memory_per_worker as usize,
                },
            )
            .await;
        }

        Ok(())
    }

    async fn fetch_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<CurrentResourceLimits, WorkerExecutorError> {
        debug!("Fetching resource limits for account {account_id}");

        let fetched_limits = self
            .client
            .get_resource_limits(account_id, &AuthCtx::System)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed fetching resource limits: {}",
                    e.to_safe_string()
                ))
            })?;

        let last_known_limits = CurrentResourceLimits {
            fuel: fetched_limits.available_fuel,
            max_memory: fetched_limits.max_memory_per_worker as usize,
        };

        self.update_last_known_limits(account_id, &last_known_limits)
            .await;

        Ok(last_known_limits)
    }

    async fn take_fuel_updates(&self) -> HashMap<AccountId, i64> {
        let mut keys = Vec::new();
        self.current_limits
            .iter_async(|k, _| {
                keys.push(*k);
                true
            })
            .await;

        let mut updates = HashMap::new();
        for k in keys {
            self.current_limits
                .update_async(&k, |_, entry| {
                    if entry.delta != 0 {
                        let to_send = if entry.delta > i64::MAX as i128 {
                            i64::MAX
                        } else if entry.delta < i64::MIN as i128 {
                            i64::MIN
                        } else {
                            entry.delta as i64
                        };

                        updates.insert(k, to_send);
                        entry.delta -= to_send as i128;
                    }
                })
                .await;
        }

        updates
    }

    async fn update_last_known_limits(
        &self,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) {
        debug!("Updating last known limits for {account_id} to {last_known_limits:?}");

        self.current_limits
            .entry_async(*account_id)
            .await
            .and_modify(|entry| {
                if entry.delta > 0 {
                    entry.limits.fuel = last_known_limits.fuel.saturating_sub(entry.delta as u64);
                } else {
                    entry.limits.fuel =
                        last_known_limits.fuel.saturating_add((-entry.delta) as u64);
                }

                entry.limits.max_memory = last_known_limits.max_memory;
            })
            .or_insert(CurrentResourceLimitsEntry {
                limits: last_known_limits.clone(),
                delta: 0,
            });
    }
}

#[async_trait]
impl ResourceLimits for ResourceLimitsGrpc {
    async fn borrow_fuel(
        &self,
        account_id: &AccountId,
        amount: u64,
    ) -> Result<u64, WorkerExecutorError> {
        loop {
            let mut borrowed = None;

            self.current_limits
                .update_async(account_id, |_, entry| {
                    let available = entry.limits.fuel.min(amount);
                    if available > 0 {
                        entry.limits.fuel -= available;
                        entry.delta = entry.delta.saturating_add(available as i128);
                        record_fuel_borrow(available);
                    }
                    borrowed = Some(available);
                })
                .await;

            match borrowed {
                Some(fuel) => break Ok(fuel),
                None => {
                    self.fetch_resource_limits(account_id).await?;
                    continue;
                }
            }
        }
    }

    fn borrow_fuel_sync(&self, account_id: &AccountId, amount: u64) -> Option<u64> {
        tokio::task::block_in_place(|| {
            let mut borrowed = None;

            self.current_limits.update_sync(account_id, |_, entry| {
                let available = entry.limits.fuel.min(amount);
                if available > 0 {
                    entry.limits.fuel -= available;
                    entry.delta = entry.delta.saturating_add(available as i128);
                    record_fuel_borrow(available);
                }
                borrowed = Some(available);
            });

            borrowed
        })
    }

    async fn return_fuel(
        &self,
        account_id: &AccountId,
        amount: u64,
    ) -> Result<(), WorkerExecutorError> {
        self.current_limits
            .update_async(account_id, |_, entry| {
                record_fuel_return(amount);
                entry.limits.fuel = entry.limits.fuel.saturating_add(amount);

                if entry.delta > 0 {
                    let delta_u64 = entry.delta.min(u64::MAX as i128) as u64;
                    let sub = amount.min(delta_u64);
                    entry.delta -= sub as i128;
                } else if entry.delta < 0 {
                    let delta_u64 = entry.delta.unsigned_abs().min(u64::MAX as u128) as u64;
                    let add_back = amount.min(delta_u64);
                    entry.delta += add_back as i128;
                }
            })
            .await;

        Ok(())
    }

    async fn get_max_memory(&self, account_id: &AccountId) -> Result<usize, WorkerExecutorError> {
        loop {
            match self
                .current_limits
                .read_async(account_id, |_, entry| entry.limits.max_memory)
                .await
            {
                Some(max_memory) => break Ok(max_memory),
                None => {
                    self.fetch_resource_limits(account_id).await?;
                    continue;
                }
            }
        }
    }
}

struct ResourceLimitsDisabled;

#[async_trait]
impl ResourceLimits for ResourceLimitsDisabled {
    async fn borrow_fuel(
        &self,
        _account_id: &AccountId,
        amount: u64,
    ) -> Result<u64, WorkerExecutorError> {
        Ok(amount)
    }

    fn borrow_fuel_sync(&self, _account_id: &AccountId, amount: u64) -> Option<u64> {
        Some(amount)
    }

    async fn return_fuel(
        &self,
        _account_id: &AccountId,
        _remaining: u64,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn get_max_memory(&self, _account_id: &AccountId) -> Result<usize, WorkerExecutorError> {
        Ok(usize::MAX)
    }
}
