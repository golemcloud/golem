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
use std::cmp::{max, min};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
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
        amount: i64,
    ) -> Result<i64, WorkerExecutorError>;

    /// Sync version of `borrow_fuel` to be used from the epoch callback.
    /// This only works if the ResourceLimits implementation is already has a cached resource limit
    /// for the given account, but this can be guaranteed by always calling `borrow_fuel` first.
    fn borrow_fuel_sync(&self, account_id: &AccountId, amount: i64) -> Option<i64>;

    /// Returns some unused fuel for a given user
    async fn return_fuel(
        &self,
        account_id: &AccountId,
        remaining: i64,
    ) -> Result<(), WorkerExecutorError>;

    /// Updates the last known resource limits for a given account
    ///
    /// This can be originating from the cloud services explicitly sending it embed into a request,
    /// or as a result of the periodic background sync.
    async fn update_last_known_limits(
        &self,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
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
        ResourceLimitsConfig::Disabled(_) => ResourceLimitsDisabled::new(),
    }
}

#[derive(Debug, Clone)]
pub struct CurrentResourceLimitsEntry {
    limits: CurrentResourceLimits,
    delta: i64,
}

/// The default ResourceLimits implementation
/// - can query the Cloud Services for information about the account's available resources
/// - caches this information for a given amount of time
/// - periodically sends batched patches to the Cloud Services to update the account's resources as
pub struct ResourceLimitsGrpc {
    client: Arc<dyn RegistryService>,
    current_limits: scc::HashMap<AccountId, CurrentResourceLimitsEntry>,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ResourceLimitsGrpc {
    async fn send_batch_updates(
        &self,
        updates: HashMap<AccountId, i64>,
    ) -> Result<(), WorkerExecutorError> {
        self.client
            .batch_update_fuel_usage(updates, &AuthCtx::System)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed updating fuel usage: {}",
                    e.to_safe_string()
                ))
            })
    }

    async fn fetch_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<CurrentResourceLimits, WorkerExecutorError> {
        let limits = self
            .client
            .get_resource_limits(account_id, &AuthCtx::System)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed fetching resource limits: {}",
                    e.to_safe_string()
                ))
            })?;
        const _: () = {
            assert!(std::mem::size_of::<usize>() == 8, "Requires 64-bit usize");
        };
        Ok(CurrentResourceLimits {
            fuel: limits.available_fuel,
            max_memory: limits.max_memory_per_worker as usize,
        })
    }

    /// Takes all recorded fuel updates and resets them to 0
    async fn take_all_fuel_updates(&self) -> HashMap<AccountId, i64> {
        let mut updates = HashMap::new();
        self.current_limits
            .iter_mut_async(|mut entry| {
                if entry.1.delta != 0 {
                    updates.insert(entry.0, entry.1.delta);
                    entry.1.delta = 0;
                }
                true
            })
            .await;

        updates
    }

    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        batch_update_interval: Duration,
    ) -> Arc<Self> {
        let svc = Self {
            client: registry_service,
            current_limits: scc::HashMap::new(),
            background_handle: Arc::new(Mutex::new(None)),
        };
        let svc = Arc::new(svc);
        let svc_clone = svc.clone();
        let background_handle = tokio::spawn(
            async move {
                loop {
                    tokio::time::sleep(batch_update_interval).await;
                    let updates = svc_clone.take_all_fuel_updates().await;
                    if !updates.is_empty() {
                        let r = svc_clone.send_batch_updates(updates.clone()).await;
                        if let Err(err) = r {
                            error!("Failed to send batched resource usage updates: {}", err);
                            error!(
                                "The following fuel consumption records were lost: {:?}",
                                updates
                            );
                        }
                    }
                }
            }
            .instrument(span!(parent: None, Level::INFO, "Resource limits batch updates")),
        );
        *svc.background_handle.lock().unwrap() = Some(background_handle);
        svc
    }
}

impl Drop for ResourceLimitsGrpc {
    fn drop(&mut self) {
        if let Some(handle) = self.background_handle.lock().unwrap().take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl ResourceLimits for ResourceLimitsGrpc {
    async fn borrow_fuel(
        &self,
        account_id: &AccountId,
        amount: i64,
    ) -> Result<i64, WorkerExecutorError> {
        loop {
            let borrowed = self.borrow_fuel_sync(account_id, amount);

            match borrowed {
                Some(fuel) => {
                    record_fuel_borrow(fuel);
                    break Ok(fuel);
                }
                None => {
                    let fetched_limits = self.fetch_resource_limits(account_id).await?;
                    self.update_last_known_limits(account_id, &fetched_limits)
                        .await?;
                    continue;
                }
            }
        }
    }

    fn borrow_fuel_sync(&self, account_id: &AccountId, amount: i64) -> Option<i64> {
        let mut borrowed = None;
        self.current_limits.update_sync(account_id, |_, entry| {
            let available = max(0, min(amount, entry.limits.fuel));
            borrowed = Some(available);
            record_fuel_borrow(available);
            entry.limits.fuel -= available;
            entry.delta -= available;
        });

        borrowed
    }

    async fn return_fuel(
        &self,
        account_id: &AccountId,
        remaining: i64,
    ) -> Result<(), WorkerExecutorError> {
        self.current_limits.update_sync(account_id, |_, entry| {
            record_fuel_return(remaining);
            entry.limits.fuel += remaining;
            entry.delta += remaining;
        });
        Ok(())
    }

    async fn update_last_known_limits(
        &self,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), WorkerExecutorError> {
        self.current_limits
            .entry_async(*account_id)
            .await
            .and_modify(|entry| {
                entry.limits.fuel = last_known_limits.fuel + entry.delta;
                entry.limits.max_memory = last_known_limits.max_memory;
            })
            .or_insert(CurrentResourceLimitsEntry {
                limits: last_known_limits.clone(),
                delta: 0,
            });
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

impl ResourceLimitsDisabled {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }
}

#[async_trait]
impl ResourceLimits for ResourceLimitsDisabled {
    async fn borrow_fuel(
        &self,
        _account_id: &AccountId,
        amount: i64,
    ) -> Result<i64, WorkerExecutorError> {
        Ok(amount)
    }

    fn borrow_fuel_sync(&self, _account_id: &AccountId, amount: i64) -> Option<i64> {
        Some(amount)
    }

    async fn return_fuel(
        &self,
        _account_id: &AccountId,
        _remaining: i64,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn update_last_known_limits(
        &self,
        _account_id: &AccountId,
        _last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn get_max_memory(&self, _account_id: &AccountId) -> Result<usize, WorkerExecutorError> {
        Ok(usize::MAX)
    }
}
