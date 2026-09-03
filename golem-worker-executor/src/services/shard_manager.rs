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

use crate::services::shard::ShardService;
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::{ShardAssignment, ShardEpoch, ShardId};
use golem_service_base::clients::shard_manager::{ShardLeaseError, ShardManagerError};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// A renewal is attempted three times inside one lease, so a single lost
/// response is not enough to lose the shards.
const RENEWAL_INTERVAL_DIVISOR: u32 = 3;

/// Floor on the derived renewal cadence, so a pathologically short lease cannot
/// turn the loop into a busy loop.
const MIN_RENEWAL_INTERVAL: Duration = Duration::from_secs(1);

/// Cadence used while the shard manager keeps refusing to renew: the lease has
/// an expiry the executor cannot extend, so it retries at the floor until it
/// either succeeds or the lease lapses and the self-fence takes over.
const RETRY_RENEWAL_INTERVAL: Duration = MIN_RENEWAL_INTERVAL;

#[async_trait]
pub trait ShardManagerService: Send + Sync {
    /// Registers this executor and returns the shard assignment the manager
    /// granted it.
    async fn register(
        &self,
        port: u16,
        pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError>;

    /// One renewal pass: assert the set this executor believes it holds, and
    /// apply whatever the manager answers. Returns how long to wait before the
    /// next pass. Public so tests can drive it without a timer, exactly as
    /// `GrpcQuotaService::renew_all` is driven.
    async fn renew_shard_lease(&self) -> Duration;

    /// Graceful release of the shard lease. Never fails a shutdown.
    async fn deregister(&self);
}

pub struct GrpcShardManagerService {
    client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
    shard_service: Arc<dyn ShardService>,
    shutdown_token: CancellationToken,
    /// The identity of this executor's shard lease. Regenerated whenever the
    /// manager answers `LeaseNotFound`, because from the manager's point of
    /// view this process is then a new instance at the same address.
    executor_id: RwLock<Uuid>,
    /// The registration arguments, kept so a re-register after `LeaseNotFound`
    /// can repeat it without going back through `WorkerExecutorImpl`.
    registration: RwLock<Option<(u16, Option<String>)>>,
    /// Weak self-reference, so `register` can spawn the renewal loop without
    /// the loop keeping this service alive.
    me: Weak<Self>,
    renewal_loop_started: AtomicBool,
}

impl GrpcShardManagerService {
    pub fn new(
        client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
        shard_service: Arc<dyn ShardService>,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            client,
            shard_service,
            shutdown_token,
            executor_id: RwLock::new(Uuid::new_v4()),
            registration: RwLock::new(None),
            me: me.clone(),
            renewal_loop_started: AtomicBool::new(false),
        })
    }

    fn executor_id(&self) -> Uuid {
        *self.executor_id.read().unwrap()
    }

    /// Structurally the quota renewal loop (`services/quota.rs:468-490`): a
    /// weak self-reference so the loop never keeps the service alive, and a
    /// `select!` over the shutdown token and the sleep. It differs in two
    /// places: the cadence is re-derived from each granted expiry rather than
    /// fixed by config, and cancellation deregisters before it breaks (there is
    /// no SIGTERM handler, so the in-process token is the only trigger).
    fn start_renewal_loop(&self, first_interval: Duration) {
        if self.renewal_loop_started.swap(true, Ordering::SeqCst) {
            return;
        }
        info!(
            renewal_interval_ms = first_interval.as_millis(),
            "Starting the shard lease renewal loop"
        );
        let svc_weak = self.me.clone();
        let shutdown_token = self.shutdown_token.clone();
        tokio::spawn(async move {
            let mut renewal_interval = first_interval;
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        if let Some(svc) = svc_weak.upgrade() {
                            svc.deregister().await;
                        }
                        break;
                    }
                    _ = tokio::time::sleep(renewal_interval) => {}
                }
                let svc = match svc_weak.upgrade() {
                    Some(svc) => svc,
                    None => {
                        info!("ShardManagerService was dropped, stopping renewal loop");
                        break;
                    }
                };
                renewal_interval = svc.renew_shard_lease().await;
            }
        });
    }

    /// Applies a granted lease and returns the cadence for the next pass.
    fn adopt_lease(
        &self,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> Duration {
        let shard_epochs: HashMap<ShardId, ShardEpoch> = shard_epochs.into_iter().collect();
        if let Err(error) = self.shard_service.update_lease(&shard_epochs, expires_at) {
            warn!(%error, "Failed to apply a renewed shard lease");
        }
        renewal_interval_for(expires_at, Utc::now())
    }
}

/// `(expires_at - now) / 3`, floored, so three attempts fit inside one lease.
/// A lease that never expires is polled at the floor; the loop is only started
/// for leases that do expire, so that case only arises if a manager stops
/// sending an expiry.
fn renewal_interval_for(
    expires_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> Duration {
    match expires_at {
        None => MIN_RENEWAL_INTERVAL,
        Some(expires_at) => (expires_at - now)
            .to_std()
            .map(|remaining| remaining / RENEWAL_INTERVAL_DIVISOR)
            .unwrap_or(MIN_RENEWAL_INTERVAL)
            .max(MIN_RENEWAL_INTERVAL),
    }
}

#[async_trait]
impl ShardManagerService for GrpcShardManagerService {
    async fn register(
        &self,
        port: u16,
        pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError> {
        *self.registration.write().unwrap() = Some((port, pod_name.clone()));

        let registration = self
            .client
            .register(port, pod_name, self.executor_id())
            .await?;

        let number_of_shards = registration.number_of_shards.try_into().map_err(|_| {
            ShardManagerError::ConversionError(format!(
                "RegisterSuccess.number_of_shards {} does not fit a usize",
                registration.number_of_shards
            ))
        })?;

        let assignment = ShardAssignment {
            number_of_shards,
            shard_epochs: registration.lease.shard_epochs.into_iter().collect(),
            expires_at: registration.lease.expires_at,
        };

        // Started here rather than by the caller because this is the first
        // point at which a lease exists, and started unconditionally so that a
        // graceful shutdown always deregisters. A real shard manager always
        // sends an expiry (cross-track contract); the `None` cadence is only
        // the floor because it cannot legitimately occur here.
        self.start_renewal_loop(renewal_interval_for(assignment.expires_at, Utc::now()));

        Ok(assignment)
    }

    async fn renew_shard_lease(&self) -> Duration {
        let claim = match self.shard_service.current_assignment() {
            Ok(assignment) => assignment.claim(),
            Err(error) => {
                warn!(%error, "Skipping shard lease renewal, no shard assignment yet");
                return RETRY_RENEWAL_INTERVAL;
            }
        };

        let executor_id = self.executor_id();
        match self.client.renew_shard_lease(executor_id, claim).await {
            Ok(lease) => self.adopt_lease(lease.shard_epochs, lease.expires_at),
            Err(ShardLeaseError::StaleEpoch(details)) => {
                // The manager's view of this executor's shards moved on. Keep
                // the current set and retry: the correction arrives as an
                // AssignShards push, not through the renewal.
                warn!(
                    details,
                    "Shard lease renewal rejected as stale, keeping the current assignment"
                );
                RETRY_RENEWAL_INTERVAL
            }
            Err(ShardLeaseError::LeaseNotFound(details)) => {
                // The manager no longer knows this executor. Drop every shard
                // (an empty set fences every agent) and come back as a new
                // instance at the same address, which is the restarted-executor
                // path the manager already handles.
                warn!(
                    details,
                    "Shard lease not found, clearing the assignment and re-registering"
                );
                self.shard_service.clear_assignment();
                let fresh_executor_id = Uuid::new_v4();
                *self.executor_id.write().unwrap() = fresh_executor_id;

                let registration = self.registration.read().unwrap().clone();
                match registration {
                    None => {
                        error!("Cannot re-register: this executor never completed a registration");
                        RETRY_RENEWAL_INTERVAL
                    }
                    Some((port, pod_name)) => match self.register(port, pod_name).await {
                        Ok(assignment) => {
                            self.shard_service.register(
                                assignment.number_of_shards,
                                &assignment.shard_epochs,
                                assignment.expires_at,
                            );
                            info!(
                                executor_id = %fresh_executor_id,
                                "Re-registered with the shard manager after a lost lease"
                            );
                            renewal_interval_for(assignment.expires_at, Utc::now())
                        }
                        Err(error) => {
                            warn!(%error, "Re-registration after a lost lease failed");
                            RETRY_RENEWAL_INTERVAL
                        }
                    },
                }
            }
            Err(error) => {
                // Transport class: nothing local changes, so the stored expiry
                // runs down on its own and the self-fence starts refusing
                // admission the moment it passes.
                warn!(%error, "Shard lease renewal failed, retrying");
                RETRY_RENEWAL_INTERVAL
            }
        }
    }

    async fn deregister(&self) {
        let claim = self
            .shard_service
            .try_get_current_assignment()
            .map(|assignment| assignment.claim())
            .unwrap_or_default();

        let executor_id = self.executor_id();
        match self.client.deregister(executor_id, claim).await {
            Ok(()) => info!(%executor_id, "Deregistered from the shard manager"),
            Err(error) => warn!(%error, "Failed to deregister from the shard manager"),
        }
        self.shard_service.clear_assignment();
    }
}

/// Single-shard implementation for local development and the debugging
/// service.  Returns a single shard assignment without contacting a real
/// shard manager.
pub struct ShardManagerServiceSingleShard;

#[async_trait]
impl ShardManagerService for ShardManagerServiceSingleShard {
    async fn register(
        &self,
        _port: u16,
        _pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError> {
        // `expires_at: None` — the single binary and the debugging service have
        // no shard manager to renew against and must never fence themselves.
        Ok(ShardAssignment::unexpiring(1, [ShardId::new(0)]))
    }

    async fn renew_shard_lease(&self) -> Duration {
        MIN_RENEWAL_INTERVAL
    }

    async fn deregister(&self) {}
}
