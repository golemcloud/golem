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
use crate::services::shutdown::Shutdown;
use async_trait::async_trait;
use chrono::Utc;
use futures::future::BoxFuture;
use golem_common::model::{
    ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision,
};
use golem_service_base::clients::shard_manager::{ShardLeaseError, ShardManagerError};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

/// A renewal is attempted three times inside one lease, so a single lost
/// response is not enough to lose the shards.
const RENEWAL_INTERVAL_DIVISOR: u32 = 3;

/// Floor on the derived renewal cadence, so a pathologically short lease cannot
/// turn the loop into a busy loop. Also the first step of the retry backoff.
const MIN_RENEWAL_INTERVAL: Duration = Duration::from_secs(1);

/// Absolute ceiling on the retry backoff, so a shard-manager
/// outage never becomes one RPC per second per executor. The effective ceiling
/// is the smaller of this and the cadence the last granted lease implied.
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the renewal loop waits before its next pass. `None` parks the loop:
/// the granted lease never expires, so there is nothing to renew and only the
/// shutdown arm can fire.
pub type RenewalDelay = Option<Duration>;

/// Fired when a re-registration or a corrected renewal replaces this executor's
/// shard assignment, so running agents are recovered for the new set exactly as
/// the initial registration and `assign_shards_internal` do. Installed by
/// `WorkerExecutorImpl::new`, which is the only place that can name `Ctx`.
pub type ShardAssignmentChangedHookFn =
    dyn Fn() -> BoxFuture<'static, Result<(), anyhow::Error>> + Send + Sync;
pub type ShardAssignmentChangedHook = Arc<ShardAssignmentChangedHookFn>;

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
    /// next pass, or `None` to park until shutdown. Public so tests can drive
    /// it without a timer, exactly as `GrpcQuotaService::renew_all` is driven.
    async fn renew_shard_lease(&self) -> RenewalDelay;

    /// Graceful release of the shard lease. Never fails a shutdown.
    async fn deregister(&self);

    /// Installs the hook fired when a re-registration or a corrected renewal
    /// replaces this executor's shard assignment. No-op by default: an
    /// implementation that never re-registers has nothing to announce.
    fn set_assignment_changed_hook(&self, _hook: &ShardAssignmentChangedHook) {}
}

/// The interval arm of the renewal loop. A `None` delay is a lease that never
/// expires: the arm is pending forever, so the loop issues no RPCs at all and
/// only the shutdown arm can fire.
async fn sleep_or_park(delay: RenewalDelay) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending().await,
    }
}

pub struct GrpcShardManagerService {
    client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
    shard_service: Arc<dyn ShardService>,
    shutdown: Shutdown,
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
    /// Exponential backoff for failed renewals: doubles from
    /// `MIN_RENEWAL_INTERVAL` up to `retry_cap()`, reset by every grant.
    retry_backoff: RwLock<Duration>,
    /// The cadence the last granted lease implied (`TTL / 3`), which caps the
    /// backoff so a retry never waits longer than the lease it is saving.
    granted_cadence: RwLock<Option<Duration>>,
    /// Fired when a re-registration or a corrected renewal replaces the shard assignment.
    ///
    /// Weak, following `LazyWorkerActivator`: the hook closes over the service graph that owns
    /// this service, so a strong reference here would be a cycle nothing could free.
    /// `WorkerExecutorImpl` holds the strong one, so the hook lives as long as the executor.
    assignment_changed_hook: RwLock<Option<Weak<ShardAssignmentChangedHookFn>>>,
    /// Set when the hook failed, so the next grant runs it again even if the set is unchanged.
    /// A push that fails this way is retried by the shard manager; a renewal has nobody to
    /// retry it, and an unchanged grant would otherwise never run it again.
    recovery_pending: AtomicBool,
}

impl GrpcShardManagerService {
    pub fn new(
        client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
        shard_service: Arc<dyn ShardService>,
        shutdown: Shutdown,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            client,
            shard_service,
            shutdown,
            executor_id: RwLock::new(Uuid::new_v4()),
            registration: RwLock::new(None),
            me: me.clone(),
            renewal_loop_started: AtomicBool::new(false),
            retry_backoff: RwLock::new(MIN_RENEWAL_INTERVAL),
            granted_cadence: RwLock::new(None),
            assignment_changed_hook: RwLock::new(None),
            recovery_pending: AtomicBool::new(false),
        })
    }

    /// How long one lease RPC may take: `retry_cap()`, so a manager that accepts the call and
    /// never answers costs a bounded wait and a backoff rather than the lease. Every call from
    /// the manager to an executor carries a deadline; this is the one in the other direction.
    fn rpc_deadline(&self) -> Duration {
        self.retry_cap()
    }

    fn executor_id(&self) -> Uuid {
        *self.executor_id.read().unwrap()
    }

    /// `min(last granted TTL / 3, 30 s)` — the ceiling the retry backoff climbs
    /// to. Before anything has been granted there is no lease to
    /// outlive, so only the absolute ceiling applies.
    fn retry_cap(&self) -> Duration {
        match *self.granted_cadence.read().unwrap() {
            Some(cadence) => cadence.min(MAX_RETRY_INTERVAL),
            None => MAX_RETRY_INTERVAL,
        }
    }

    /// The delay after a failed renewal: the current backoff, which then
    /// doubles for the next failure, capped by `retry_cap()`.
    fn next_retry_delay(&self) -> RenewalDelay {
        let cap = self.retry_cap();
        let mut backoff = self.retry_backoff.write().unwrap();
        let delay = (*backoff).min(cap);
        *backoff = delay.saturating_mul(2).min(cap);
        Some(delay)
    }

    /// A granted lease resets the backoff to its first step and records the
    /// cadence that caps it.
    fn record_granted(&self, cadence: RenewalDelay) {
        *self.retry_backoff.write().unwrap() = MIN_RENEWAL_INTERVAL;
        *self.granted_cadence.write().unwrap() = cadence;
    }

    /// Tell the executor its shard assignment changed, so running
    /// agents are recovered for the new set. A failure here is logged, never
    /// fatal — the lease itself is already installed — and remembered, so the
    /// next grant runs the recovery again.
    async fn announce_assignment_changed(&self) {
        let hook = self
            .assignment_changed_hook
            .read()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade);
        let Some(hook) = hook else {
            return;
        };
        match hook().await {
            Ok(()) => self.recovery_pending.store(false, Ordering::SeqCst),
            Err(error) => {
                self.recovery_pending.store(true, Ordering::SeqCst);
                warn!(%error, "Recovering agents for the shard set failed; retrying on the next grant");
            }
        }
    }

    /// Structurally the quota renewal loop (`GrpcQuotaService::start_renewal_loop`): a
    /// weak self-reference so the loop never keeps the service alive, and a
    /// `select!` over the shutdown token and the sleep. It differs in two
    /// places: the cadence is re-derived from each granted expiry rather than
    /// fixed by config, and cancellation deregisters before it breaks (the
    /// executor's `main` trips the token on a termination signal and waits for
    /// this task, so a rolling deploy re-homes shards at once).
    fn start_renewal_loop(&self, first_delay: RenewalDelay) {
        if self.renewal_loop_started.swap(true, Ordering::SeqCst) {
            return;
        }
        info!(
            renewal_interval_ms = first_delay.map(|delay| delay.as_millis()),
            "Starting the shard lease renewal loop"
        );
        let svc_weak = self.me.clone();
        let shutdown_token = self.shutdown.token();
        // Through the shutdown tracker rather than `tokio::spawn`: the shutdown
        // arm below deregisters, and `main` waits for tracked tasks so that RPC
        // lands before the runtime is torn down.
        self.shutdown.spawn(async move {
            let mut renewal_delay = first_delay;
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        if let Some(svc) = svc_weak.upgrade() {
                            svc.deregister().await;
                        }
                        break;
                    }
                    // `None` parks here forever, so a never-expiring lease
                    // issues no renewal RPCs at all.
                    _ = sleep_or_park(renewal_delay) => {}
                }
                let svc = match svc_weak.upgrade() {
                    Some(svc) => svc,
                    None => {
                        info!("ShardManagerService was dropped, stopping renewal loop");
                        break;
                    }
                };
                // Raced against the token as well, not just the sleep before it: a renewal can
                // be followed by a re-registration and a full agent recovery, and a termination
                // signal arriving meanwhile has to be seen inside the grace `main` waits, which
                // is the window the deregister below must be sent in. Abandoning the renewal
                // costs nothing: the process is stopping and handing the lease back.
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        svc.deregister().await;
                        break;
                    }
                    delay = svc.renew_shard_lease() => renewal_delay = delay,
                }
            }
        });
    }

    /// Applies a granted lease and returns the cadence for the next pass.
    ///
    /// The grant is the shard manager's set for this executor. Normally that
    /// is exactly what was claimed, and only the lease clock moves. When it is
    /// not, the manager is correcting a push this executor never received,
    /// wider or narrower, and it is an assignment change like any other: the
    /// hook runs the same sweep-and-recover the push path does. A grant older
    /// than the last delivery applied crossed with a push on the network and
    /// is ignored whole, or it would put the older set back.
    async fn adopt_lease(
        &self,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
        expires_at: Option<chrono::DateTime<Utc>>,
        revision: ShardLeaseRevision,
    ) -> RenewalDelay {
        let shard_epochs: HashMap<ShardId, ShardEpoch> = shard_epochs.into_iter().collect();
        match self
            .shard_service
            .update_lease(&shard_epochs, expires_at, revision)
        {
            Ok(ShardDeliveryOutcome::Applied { set_changed: true }) => {
                info!(
                    %revision,
                    "Shard lease renewal corrected the shard set; sweeping and recovering agents"
                );
                self.announce_assignment_changed().await
            }
            Ok(ShardDeliveryOutcome::Applied { set_changed: false }) => {
                if self.recovery_pending.load(Ordering::SeqCst) {
                    info!(%revision, "Retrying the agent recovery that failed on the last grant");
                    self.announce_assignment_changed().await
                }
            }
            Ok(ShardDeliveryOutcome::Stale { delivered, applied }) => warn!(
                %delivered,
                %applied,
                "Ignoring a shard lease renewal older than the last delivery applied"
            ),
            Err(error) => warn!(%error, "Failed to apply a renewed shard lease"),
        }
        let cadence = renewal_interval_for(expires_at, Utc::now());
        self.record_granted(cadence);
        cadence
    }
}

/// `(expires_at - now) / 3`, floored, so three attempts fit inside one lease.
///
/// A lease that never expires yields `None`, which parks the
/// renewal loop instead of polling it — there is nothing to renew, and a
/// polling loop would be one wasted RPC per second per executor.
fn renewal_interval_for(
    expires_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> RenewalDelay {
    let expires_at = expires_at?;
    Some(
        (expires_at - now)
            .to_std()
            .map(|remaining| remaining / RENEWAL_INTERVAL_DIVISOR)
            .unwrap_or(MIN_RENEWAL_INTERVAL)
            .max(MIN_RENEWAL_INTERVAL),
    )
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

        let number_of_shards: usize = registration.number_of_shards.try_into().map_err(|_| {
            ShardManagerError::ConversionError(format!(
                "RegisterSuccess.number_of_shards {} does not fit a usize",
                registration.number_of_shards
            ))
        })?;

        // `ShardId::from_agent_id` divides by it, so a zero would abort this executor on its
        // first routing decision rather than fail the registration. `AssignShards` refuses one
        // for the same reason; this is the other door the count comes through.
        if number_of_shards == 0 {
            return Err(ShardManagerError::ConversionError(
                "RegisterSuccess.number_of_shards must not be 0".to_string(),
            ));
        }

        let assignment = ShardAssignment {
            number_of_shards,
            shard_epochs: registration.lease.shard_epochs.into_iter().collect(),
            expires_at: registration.lease.expires_at,
            revision: registration.lease.revision,
        };

        // Started here rather than by the caller because this is the first
        // point at which a lease exists, and started unconditionally so that a
        // graceful shutdown always deregisters (a termination signal trips the
        // same token). A grant with no expiry parks the loop on its shutdown arm
        // and issues no RPCs at all.
        let cadence = renewal_interval_for(assignment.expires_at, Utc::now());
        self.record_granted(cadence);
        self.start_renewal_loop(cadence);

        Ok(assignment)
    }

    async fn renew_shard_lease(&self) -> RenewalDelay {
        let claim = match self.shard_service.current_assignment() {
            Ok(assignment) => assignment.claim(),
            Err(error) => {
                warn!(%error, "Skipping shard lease renewal, no shard assignment yet");
                return self.next_retry_delay();
            }
        };

        let executor_id = self.executor_id();
        let deadline = self.rpc_deadline();
        let renewed =
            match tokio::time::timeout(deadline, self.client.renew_shard_lease(executor_id, claim))
                .await
            {
                Ok(renewed) => renewed,
                Err(_elapsed) => {
                    warn!(
                        deadline_ms = deadline.as_millis(),
                        "Shard lease renewal did not answer in time; backing off"
                    );
                    return self.next_retry_delay();
                }
            };
        match renewed {
            Ok(lease) => {
                self.adopt_lease(lease.shard_epochs, lease.expires_at, lease.revision)
                    .await
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
                        self.next_retry_delay()
                    }
                    Some((port, pod_name)) => match self.register(port, pod_name).await {
                        Ok(assignment) => {
                            self.shard_service.register(
                                assignment.number_of_shards,
                                &assignment.shard_epochs,
                                assignment.expires_at,
                                assignment.revision,
                            );
                            info!(
                                executor_id = %fresh_executor_id,
                                "Re-registered with the shard manager after a lost lease"
                            );
                            // The same announcement the initial
                            // registration and `assign_shards_internal` make.
                            self.announce_assignment_changed().await;
                            renewal_interval_for(assignment.expires_at, Utc::now())
                        }
                        Err(error) => {
                            warn!(%error, "Re-registration after a lost lease failed");
                            self.next_retry_delay()
                        }
                    },
                }
            }
            Err(error) => {
                // Transport class: nothing local changes, so the stored expiry
                // runs down on its own and the self-fence starts refusing
                // admission the moment it passes.
                warn!(%error, "Shard lease renewal failed, retrying");
                self.next_retry_delay()
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
        match tokio::time::timeout(
            self.rpc_deadline(),
            self.client.deregister(executor_id, claim),
        )
        .await
        {
            Ok(Ok(())) => info!(%executor_id, "Deregistered from the shard manager"),
            Ok(Err(error)) => warn!(%error, "Failed to deregister from the shard manager"),
            Err(_elapsed) => warn!(
                %executor_id,
                "Deregistering from the shard manager did not answer in time; the lease will lapse"
            ),
        }
        self.shard_service.clear_assignment();
    }

    fn set_assignment_changed_hook(&self, hook: &ShardAssignmentChangedHook) {
        *self.assignment_changed_hook.write().unwrap() = Some(Arc::downgrade(hook));
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

    /// No lease to renew, and no expiry to poll against: the loop this service
    /// never starts would park here anyway.
    async fn renew_shard_lease(&self) -> RenewalDelay {
        None
    }

    async fn deregister(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::shard::ShardServiceDefault;
    use chrono::{DateTime, Duration as ChronoDuration};
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::quota::{ResourceDefinitionId, ResourceName};
    use golem_common::model::{AgentId, RoutingTable};
    use golem_service_base::clients::shard_manager::{
        BatchRenewalEntry, QuotaError, ShardLease, ShardManager, ShardRegistration,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::quota_lease::{PendingReservation, QuotaLease};
    use std::collections::HashSet;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    use test_r::test;

    test_r::enable!();

    const SHARDS: usize = 8;
    const PORT: u16 = 9000;

    fn epochs(entries: impl IntoIterator<Item = (i64, u64)>) -> HashMap<ShardId, ShardEpoch> {
        entries
            .into_iter()
            .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
            .collect()
    }

    fn claim(entries: impl IntoIterator<Item = (i64, u64)>) -> BTreeMap<ShardId, ShardEpoch> {
        entries
            .into_iter()
            .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
            .collect()
    }

    /// An agent id that routes to `shard`, found by search because
    /// `ShardId::from_agent_id` is a hash.
    fn agent_on_shard(shard: i64) -> AgentId {
        let component_id = ComponentId(Uuid::nil());
        for candidate in 0..10_000 {
            let agent_id = AgentId {
                component_id,
                agent_id: format!("agent-{candidate}"),
            };
            if ShardId::from_agent_id(&agent_id, SHARDS) == ShardId::new(shard) {
                return agent_id;
            }
        }
        panic!("no agent id in the search space routes to shard {shard}");
    }

    fn registration(
        expires_at: Option<DateTime<Utc>>,
        shard_epochs: impl IntoIterator<Item = (i64, u64)>,
    ) -> ShardRegistration {
        ShardRegistration {
            number_of_shards: SHARDS as u32,
            lease: ShardLease {
                shard_epochs: claim(shard_epochs),
                expires_at,
                revision: ShardLeaseRevision(1),
            },
        }
    }

    type RegisterFn =
        Box<dyn Fn(Uuid) -> Result<ShardRegistration, ShardManagerError> + Send + Sync>;
    type RenewFn = Box<
        dyn Fn(Uuid, BTreeMap<ShardId, ShardEpoch>) -> Result<ShardLease, ShardLeaseError>
            + Send
            + Sync,
    >;

    /// The `ShardManager` client double, modelled on the `MockShardManager`
    /// harness in `services/quota.rs`: every quota method is unreachable here,
    /// and the three shard-lease methods are scripted and recorded.
    struct MockShardManager {
        register_fn: StdMutex<Option<RegisterFn>>,
        renew_fn: StdMutex<Option<RenewFn>>,
        /// Holds a renewal inside the RPC, so a test can act while one is still in flight.
        renew_gate: StdMutex<Option<Arc<tokio::sync::Notify>>>,
        register_calls: StdMutex<Vec<Uuid>>,
        renew_calls: StdMutex<Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)>>,
        deregister_calls: StdMutex<Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)>>,
    }

    impl MockShardManager {
        fn new() -> Self {
            Self {
                register_fn: StdMutex::new(None),
                renew_fn: StdMutex::new(None),
                renew_gate: StdMutex::new(None),
                register_calls: StdMutex::new(Vec::new()),
                renew_calls: StdMutex::new(Vec::new()),
                deregister_calls: StdMutex::new(Vec::new()),
            }
        }

        fn with_register(
            self,
            f: impl Fn(Uuid) -> Result<ShardRegistration, ShardManagerError> + Send + Sync + 'static,
        ) -> Self {
            *self.register_fn.lock().unwrap() = Some(Box::new(f));
            self
        }

        fn with_renew(
            self,
            f: impl Fn(Uuid, BTreeMap<ShardId, ShardEpoch>) -> Result<ShardLease, ShardLeaseError>
            + Send
            + Sync
            + 'static,
        ) -> Self {
            *self.renew_fn.lock().unwrap() = Some(Box::new(f));
            self
        }

        fn with_renew_gate(self, gate: Arc<tokio::sync::Notify>) -> Self {
            *self.renew_gate.lock().unwrap() = Some(gate);
            self
        }

        fn register_calls(&self) -> Vec<Uuid> {
            self.register_calls.lock().unwrap().clone()
        }

        fn renew_calls(&self) -> Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)> {
            self.renew_calls.lock().unwrap().clone()
        }

        fn deregister_calls(&self) -> Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)> {
            self.deregister_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ShardManager for MockShardManager {
        async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
            unimplemented!()
        }

        async fn register(
            &self,
            _port: u16,
            _pod_name: Option<String>,
            executor_id: Uuid,
        ) -> Result<ShardRegistration, ShardManagerError> {
            self.register_calls.lock().unwrap().push(executor_id);
            let guard = self.register_fn.lock().unwrap();
            let f = guard.as_ref().expect("register_fn not configured");
            f(executor_id)
        }

        async fn renew_shard_lease(
            &self,
            executor_id: Uuid,
            shard_epochs: BTreeMap<ShardId, ShardEpoch>,
        ) -> Result<ShardLease, ShardLeaseError> {
            self.renew_calls
                .lock()
                .unwrap()
                .push((executor_id, shard_epochs.clone()));
            // Cloned out before the await: the guard must not be held across it.
            let gate = self.renew_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            let guard = self.renew_fn.lock().unwrap();
            let f = guard.as_ref().expect("renew_fn not configured");
            f(executor_id, shard_epochs)
        }

        async fn deregister(
            &self,
            executor_id: Uuid,
            shard_epochs: BTreeMap<ShardId, ShardEpoch>,
        ) -> Result<(), ShardLeaseError> {
            self.deregister_calls
                .lock()
                .unwrap()
                .push((executor_id, shard_epochs));
            Ok(())
        }

        async fn acquire_quota_lease(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
            _port: u16,
        ) -> Result<QuotaLease, QuotaError> {
            unimplemented!()
        }

        async fn renew_quota_lease(
            &self,
            _resource_definition_id: ResourceDefinitionId,
            _port: u16,
            _epoch: u64,
            _unused: u64,
            _pending_reservations: Vec<PendingReservation>,
        ) -> Result<QuotaLease, QuotaError> {
            unimplemented!()
        }

        async fn batch_renew_quota_leases(
            &self,
            _port: u16,
            _renewals: Vec<BatchRenewalEntry>,
        ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError> {
            unimplemented!()
        }

        async fn release_quota_lease(
            &self,
            _resource_definition_id: ResourceDefinitionId,
            _port: u16,
            _epoch: u64,
            _unused: u64,
        ) -> Result<(), QuotaError> {
            unimplemented!()
        }
    }

    /// A service wired to the mock, with no renewal loop running: every test
    /// drives `renew_shard_lease()` itself, exactly as the quota tests drive
    /// `renew_all()`.
    fn make_service(
        mock: Arc<MockShardManager>,
        shutdown: Shutdown,
    ) -> (Arc<GrpcShardManagerService>, Arc<ShardServiceDefault>) {
        let shard_service = Arc::new(ShardServiceDefault::new());
        let service = GrpcShardManagerService::new(mock, shard_service.clone(), shutdown);
        (service, shard_service)
    }

    #[test]
    // The hook closes over the service graph that owns this service, so a strong reference here
    // would be a cycle nothing could free. `WorkerExecutorImpl` owns the hook; this only borrows
    // it.
    async fn the_service_only_borrows_the_assignment_changed_hook() {
        let mock = Arc::new(MockShardManager::new());
        let (service, _shard_service) = make_service(mock, Shutdown::new());

        let owner = Arc::new(());
        let sentinel = Arc::downgrade(&owner);
        let hook: ShardAssignmentChangedHook = Arc::new(move || {
            let _captured = owner.clone();
            Box::pin(async move { Ok(()) })
        });
        service.set_assignment_changed_hook(&hook);
        assert!(
            sentinel.upgrade().is_some(),
            "the hook is alive while its owner holds it"
        );

        drop(hook);
        assert!(
            sentinel.upgrade().is_none(),
            "the shard manager service must not keep the assignment-changed hook alive - holding \
             it would keep the whole service graph alive with it"
        );

        // And an announcement with nothing to announce to is simply skipped, never a panic.
        service.announce_assignment_changed().await;
    }

    #[test]
    // The loop watches the shutdown token while it sleeps *and* while a renewal is in flight. A
    // renewal is a round trip that can be followed by a re-registration and a recovery of every
    // running agent, so a token only looked at between renewals would let a termination signal
    // wait out the grace `main` allows - and the lease would then be handed back by expiry, a
    // whole lease period later, rather than by the deregister this loop exists to send.
    async fn a_shutdown_during_an_in_flight_renewal_still_deregisters() {
        let expiry = Utc::now() + ChronoDuration::seconds(3);
        // Never released: the renewal is still in the RPC when the token is cancelled.
        let gate = Arc::new(tokio::sync::Notify::new());
        let mock = Arc::new(
            MockShardManager::new()
                .with_register(move |_| Ok(registration(Some(expiry), [(0, 1)])))
                .with_renew(move |_, claimed| {
                    Ok(ShardLease {
                        shard_epochs: claimed,
                        expires_at: Some(expiry),
                        revision: ShardLeaseRevision(1),
                    })
                })
                .with_renew_gate(gate.clone()),
        );
        let shutdown = Shutdown::new();
        let (service, shard_service) = make_service(mock.clone(), shutdown.clone());

        let assignment = service.register(PORT, None).await.unwrap();
        shard_service.register(
            assignment.number_of_shards,
            &assignment.shard_epochs,
            assignment.expires_at,
            assignment.revision,
        );

        // The renewal has been sent and is parked in the gate.
        for _ in 0..100 {
            if !mock.renew_calls().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            mock.renew_calls().len(),
            1,
            "the loop should have issued a renewal that is now in flight"
        );
        assert!(mock.deregister_calls().is_empty());

        shutdown.cancel();

        for _ in 0..100 {
            if !mock.deregister_calls().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            mock.deregister_calls().len(),
            1,
            "cancellation must be observed while the renewal is still in flight, not after it"
        );
    }

    #[test]
    // A shard count of zero would reach `ShardId::from_agent_id`, which divides by it, on the
    // first routing decision. The registration refuses it, so a misconfigured manager fails this
    // executor at startup rather than aborting it later - and installs nothing.
    async fn a_registration_granting_zero_shards_is_refused() {
        let expiry = Utc::now() + ChronoDuration::seconds(60);
        let mock = Arc::new(MockShardManager::new().with_register(move |_| {
            Ok(ShardRegistration {
                number_of_shards: 0,
                lease: ShardLease {
                    shard_epochs: BTreeMap::new(),
                    expires_at: Some(expiry),
                    revision: ShardLeaseRevision(1),
                },
            })
        }));
        let (service, shard_service) = make_service(mock, Shutdown::new());

        let result = service.register(PORT, None).await;

        assert!(
            matches!(&result, Err(ShardManagerError::ConversionError(message)) if message.contains("must not be 0")),
            "a zero shard count must be refused, got {result:?}"
        );
        assert!(
            shard_service.try_get_current_assignment().is_none(),
            "a refused registration must install nothing"
        );
    }

    #[test]
    // A manager that accepts the renewal and never answers must not hold the loop: the RPC has a
    // deadline derived from the lease, after which the pass counts as failed and backs off, so
    // the loop keeps renewing (and can still re-register) instead of parking until the transport
    // gives up while the lease lapses under it.
    async fn a_renewal_that_never_answers_times_out_and_the_loop_keeps_going() {
        let expiry = Utc::now() + ChronoDuration::seconds(3);
        // Never released: every renewal parks in the RPC until its deadline.
        let gate = Arc::new(tokio::sync::Notify::new());
        let mock = Arc::new(
            MockShardManager::new()
                .with_register(move |_| Ok(registration(Some(expiry), [(0, 1)])))
                .with_renew(move |_, claimed| {
                    Ok(ShardLease {
                        shard_epochs: claimed,
                        expires_at: Some(expiry),
                        revision: ShardLeaseRevision(1),
                    })
                })
                .with_renew_gate(gate.clone()),
        );
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());

        let assignment = service.register(PORT, None).await.unwrap();
        shard_service.register(
            assignment.number_of_shards,
            &assignment.shard_epochs,
            assignment.expires_at,
            assignment.revision,
        );

        // Cadence 1 s, deadline 1 s, first backoff 1 s: a second renewal within a few seconds is
        // only possible if the first one was given up on.
        for _ in 0..160 {
            if mock.renew_calls().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            mock.renew_calls().len() >= 2,
            "a renewal that never answers must time out so the loop can try again"
        );
    }

    #[test]
    // A push whose recovery fails is retried by the manager; a corrected renewal has nobody to
    // retry it, so the service remembers the failure and runs the recovery again on the next
    // grant even though that grant changes nothing - and stops once it has succeeded.
    async fn a_failed_recovery_is_retried_on_the_next_grant_even_if_the_set_is_unchanged() {
        let expiry = Utc::now() + ChronoDuration::seconds(300);
        let mock = Arc::new(MockShardManager::new().with_renew(move |_, _| {
            Ok(ShardLease {
                shard_epochs: claim([(0, 1), (1, 1)]),
                expires_at: Some(expiry),
                revision: ShardLeaseRevision(2),
            })
        }));
        let (service, shard_service) = make_service(mock, Shutdown::new());
        shard_service.register(
            SHARDS,
            &epochs([(0, 1)]),
            Some(expiry),
            ShardLeaseRevision(1),
        );

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_calls = calls.clone();
        let hook: ShardAssignmentChangedHook = Arc::new(move || {
            let hook_calls = hook_calls.clone();
            Box::pin(async move {
                let call = hook_calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    Err(anyhow::anyhow!("recovery failed"))
                } else {
                    Ok(())
                }
            })
        });
        service.set_assignment_changed_hook(&hook);

        // The corrected set: the hook runs and fails.
        service.renew_shard_lease().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Unchanged grant: the failed recovery is retried, and succeeds.
        service.renew_shard_lease().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Unchanged grant, nothing pending: the hook stays quiet.
        service.renew_shard_lease().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a recovery that succeeded must not be run again for an unchanged set"
        );
    }

    /// A grant that never expires has nothing to renew, so the
    /// loop's interval arm is pending forever and only shutdown can fire.
    /// Deregister-on-shutdown must still work — it is what a termination signal
    /// ends up triggering.
    #[test]
    async fn a_never_expiring_grant_parks_the_loop_and_still_deregisters() {
        assert_eq!(
            renewal_interval_for(None, Utc::now()),
            None,
            "a never-expiring lease must not be polled at all"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), sleep_or_park(None))
                .await
                .is_err(),
            "the parked interval arm must never fire"
        );

        let mock =
            Arc::new(MockShardManager::new().with_register(|_| Ok(registration(None, [(0, 0)]))));
        let shutdown = Shutdown::new();
        let (service, shard_service) = make_service(mock.clone(), shutdown.clone());

        let assignment = service.register(PORT, None).await.unwrap();
        shard_service.register(
            assignment.number_of_shards,
            &assignment.shard_epochs,
            assignment.expires_at,
            assignment.revision,
        );

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            mock.renew_calls().is_empty(),
            "a never-expiring lease must issue no renewal RPCs"
        );

        shutdown.cancel();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            mock.deregister_calls().len(),
            1,
            "shutdown must still release the lease"
        );
    }

    /// Cross-track contract: `RenewShardLeaseRequest.shard_epochs` is exactly
    /// the set last received, and the granted expiry replaces the local one.
    #[test]
    async fn a_renewal_claims_the_last_received_set_and_adopts_the_granted_expiry() {
        let granted_expiry = Utc::now() + ChronoDuration::seconds(300);
        let mock = Arc::new(MockShardManager::new().with_renew(move |_, claimed| {
            Ok(ShardLease {
                shard_epochs: claimed,
                expires_at: Some(granted_expiry),
                revision: ShardLeaseRevision(1),
            })
        }));
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
        shard_service.register(
            SHARDS,
            &epochs([(0, 7), (3, 2)]),
            Some(Utc::now() + ChronoDuration::seconds(10)),
            ShardLeaseRevision(1),
        );

        let delay = service.renew_shard_lease().await;

        let calls = mock.renew_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].1,
            claim([(0, 7), (3, 2)]),
            "the claim must be exactly the set last received, epochs included"
        );
        let assignment = shard_service.current_assignment().unwrap();
        assert_eq!(assignment.expires_at, Some(granted_expiry));
        assert_eq!(assignment.shard_epochs, epochs([(0, 7), (3, 2)]));
        let delay = delay.expect("a granted lease has a cadence");
        assert!(
            delay > Duration::from_secs(90) && delay <= Duration::from_secs(100),
            "the next pass is one third of the granted TTL, got {delay:?}"
        );
    }

    /// An unknown lease clears the assignment (leaving
    /// it lapsed), re-registers under a fresh UUID, and announces the new
    /// assignment so running agents are recovered.
    #[test]
    async fn a_lost_lease_re_registers_with_a_fresh_uuid_and_announces_the_new_assignment() {
        let fresh_expiry = Utc::now() + ChronoDuration::seconds(120);
        let mock = Arc::new(
            MockShardManager::new()
                .with_register(move |_| Ok(registration(Some(fresh_expiry), [(2, 5)])))
                .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
        );
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());

        let announced = Arc::new(AtomicBool::new(false));
        let flag = announced.clone();
        let hook: ShardAssignmentChangedHook = Arc::new(move || {
            let flag = flag.clone();
            Box::pin(async move {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        service.set_assignment_changed_hook(&hook);

        let assignment = service.register(PORT, None).await.unwrap();
        shard_service.register(
            assignment.number_of_shards,
            &assignment.shard_epochs,
            assignment.expires_at,
            assignment.revision,
        );
        let original_executor_id = mock.register_calls()[0];

        service.renew_shard_lease().await;

        let register_calls = mock.register_calls();
        assert_eq!(register_calls.len(), 2, "a lost lease must re-register");
        assert_ne!(
            register_calls[1], original_executor_id,
            "the re-registration must come back as a new instance, under a fresh UUID"
        );
        assert!(
            announced.load(Ordering::SeqCst),
            "the fresh grant must fire on_shard_assignment_changed"
        );
        let assignment = shard_service.current_assignment().unwrap();
        assert_eq!(assignment.shard_epochs, epochs([(2, 5)]));
        assert_eq!(assignment.expires_at, Some(fresh_expiry));
    }

    /// A renewal that answers with shards this executor did not claim is the
    /// shard manager correcting a push that never arrived, so it has to recover
    /// agents for them exactly as a push would. The common path — the same set
    /// back, because a renewal never advances an epoch — must NOT fire the
    /// hook, or every renewal would trigger a
    /// recovery sweep.
    #[test]
    async fn a_renewal_that_changes_the_set_announces_it_and_an_unchanged_one_does_not() {
        let granted_expiry = Utc::now() + ChronoDuration::seconds(300);
        // `None` echoes the claim; `Some` is the manager correcting it.
        let correction: Arc<StdMutex<Option<BTreeMap<ShardId, ShardEpoch>>>> =
            Arc::new(StdMutex::new(None));
        let correct = correction.clone();
        let mock = Arc::new(MockShardManager::new().with_renew(move |_, claimed| {
            let shard_epochs = correct.lock().unwrap().clone().unwrap_or(claimed);
            Ok(ShardLease {
                shard_epochs,
                expires_at: Some(granted_expiry),
                revision: ShardLeaseRevision(1),
            })
        }));
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
        shard_service.register(
            SHARDS,
            &epochs([(0, 7), (3, 2)]),
            Some(Utc::now() + ChronoDuration::seconds(30)),
            ShardLeaseRevision(1),
        );

        let announced = Arc::new(AtomicUsize::new(0));
        let count = announced.clone();
        let hook: ShardAssignmentChangedHook = Arc::new(move || {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        service.set_assignment_changed_hook(&hook);

        service.renew_shard_lease().await;
        assert_eq!(
            announced.load(Ordering::SeqCst),
            0,
            "an unchanged set is a lease-clock refresh, not an assignment change"
        );

        // wider: a shard this executor never knew it owned
        *correction.lock().unwrap() = Some(claim([(0, 7), (3, 2), (4, 9)]));
        service.renew_shard_lease().await;
        assert_eq!(
            announced.load(Ordering::SeqCst),
            1,
            "a widened set must recover agents for the shards it just gained"
        );
        assert_eq!(
            shard_service.current_assignment().unwrap().shard_epochs,
            epochs([(0, 7), (3, 2), (4, 9)])
        );

        // narrower: a shard the manager has given to someone else
        *correction.lock().unwrap() = Some(claim([(0, 7), (4, 9)]));
        service.renew_shard_lease().await;
        assert_eq!(
            announced.load(Ordering::SeqCst),
            2,
            "a narrowed set must sweep the agents on the shard it just lost"
        );
        assert_eq!(
            shard_service.current_assignment().unwrap().shard_epochs,
            epochs([(0, 7), (4, 9)]),
            "and the narrower set is what the executor now serves"
        );
    }

    /// Two deliveries can cross on the network. A renewal response read from
    /// an older state than a push the executor has already applied must be
    /// ignored whole, or it would put the older set back and, worse, announce
    /// that as a change.
    #[test]
    async fn a_renewal_older_than_the_last_applied_delivery_is_ignored() {
        let mock = Arc::new(MockShardManager::new().with_renew(move |_, _| {
            Ok(ShardLease {
                shard_epochs: claim([(0, 7)]),
                expires_at: Some(Utc::now() + ChronoDuration::seconds(300)),
                revision: ShardLeaseRevision(4),
            })
        }));
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
        // the push at revision 5 landed first and widened the set
        let pushed_expiry = Some(Utc::now() + ChronoDuration::seconds(30));
        shard_service.register(
            SHARDS,
            &epochs([(0, 7), (4, 9)]),
            pushed_expiry,
            ShardLeaseRevision(5),
        );

        let announced = Arc::new(AtomicBool::new(false));
        let flag = announced.clone();
        let hook: ShardAssignmentChangedHook = Arc::new(move || {
            let flag = flag.clone();
            Box::pin(async move {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        service.set_assignment_changed_hook(&hook);

        service.renew_shard_lease().await;

        let assignment = shard_service.current_assignment().unwrap();
        assert_eq!(
            assignment.shard_epochs,
            epochs([(0, 7), (4, 9)]),
            "the older renewal narrowed the set the newer push had just widened"
        );
        assert_eq!(
            assignment.expires_at, pushed_expiry,
            "ignored whole, expiry included"
        );
        assert_eq!(assignment.revision, ShardLeaseRevision(5));
        assert!(
            !announced.load(Ordering::SeqCst),
            "an ignored delivery is not an assignment change"
        );
    }

    /// While the re-registration has not succeeded, the cleared
    /// assignment must read as *lapsed*, not as never-expiring, so admission
    /// keeps refusing.
    #[test]
    async fn a_lost_lease_leaves_the_assignment_lapsed_until_a_grant_replaces_it() {
        let mock = Arc::new(
            MockShardManager::new()
                .with_register(|_| {
                    Err(ShardManagerError::InternalServerError(
                        "shard manager down".to_string(),
                    ))
                })
                .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
        );
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
        // Registration arguments the re-register repeats; the client fails, so
        // nothing replaces the cleared assignment.
        assert!(service.register(PORT, None).await.is_err());
        shard_service.register(
            SHARDS,
            &epochs([(0, 1)]),
            Some(Utc::now() + ChronoDuration::seconds(60)),
            ShardLeaseRevision(1),
        );

        service.renew_shard_lease().await;

        let assignment = shard_service.current_assignment().unwrap();
        assert!(assignment.is_empty(), "every shard is dropped");
        assert!(
            assignment.expires_at.is_some(),
            "cleared means lapsed, never 'never expires'"
        );
        assert!(!shard_service.is_ready());
        assert!(matches!(
            shard_service.check_admission(&agent_on_shard(0)),
            Err(WorkerExecutorError::ShardingNotReady)
        ));
    }

    /// A transport failure changes nothing locally, so the executor keeps
    /// serving until its own `expires_at` passes and then fences itself.
    #[test]
    async fn a_transport_failure_serves_until_the_local_expiry_and_then_fences() {
        let mock = Arc::new(MockShardManager::new().with_renew(|_, _| {
            Err(ShardLeaseError::InternalServerError(
                "unreachable".to_string(),
            ))
        }));
        let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
        let agent = agent_on_shard(0);
        // The window has to outlast a descheduling of this thread under a loaded
        // test run: the assert below is only meaningful while the granted lease is
        // still live, so it gets a second of slack.
        shard_service.register(
            SHARDS,
            &epochs([(0, 1)]),
            Some(Utc::now() + ChronoDuration::seconds(1)),
            ShardLeaseRevision(1),
        );

        let first = service.renew_shard_lease().await;
        assert_eq!(first, Some(Duration::from_secs(1)));
        assert!(
            shard_service.check_admission(&agent).is_ok(),
            "still inside the lease the executor was granted"
        );

        tokio::time::sleep(Duration::from_millis(1300)).await;

        let second = service.renew_shard_lease().await;
        assert_eq!(second, Some(Duration::from_secs(2)), "and it keeps trying");
        assert_eq!(mock.renew_calls().len(), 2);
        assert!(
            matches!(
                shard_service.check_admission(&agent),
                Err(WorkerExecutorError::ShardingNotReady)
            ),
            "past the local expiry the self-fence refuses admission"
        );
        assert_eq!(
            shard_service.current_assignment().unwrap().shard_id_set(),
            HashSet::from([ShardId::new(0)]),
            "without dropping the shards: the fence refuses new work and leaves the set alone"
        );
    }

    /// Failed renewals back off exponentially from 1 s, capped by
    /// `min(last granted TTL / 3, 30 s)`, and a grant resets the backoff.
    #[test]
    async fn failed_renewals_back_off_exponentially_and_reset_on_a_grant() {
        // Fails until told otherwise; the grant it then hands out has a 30 s
        // TTL, so the cap afterwards is that TTL / 3 rather than the ceiling.
        let grant_at_call = Arc::new(AtomicUsize::new(8));
        let calls = Arc::new(AtomicUsize::new(0));
        let grant_at = grant_at_call.clone();
        let seen = calls.clone();
        let mock = Arc::new(MockShardManager::new().with_renew(move |_, claimed| {
            let call = seen.fetch_add(1, Ordering::SeqCst) + 1;
            if call == grant_at.load(Ordering::SeqCst) {
                Ok(ShardLease {
                    shard_epochs: claimed,
                    expires_at: Some(Utc::now() + ChronoDuration::seconds(30)),
                    revision: ShardLeaseRevision(1),
                })
            } else {
                Err(ShardLeaseError::InternalServerError("down".to_string()))
            }
        }));
        let (service, shard_service) = make_service(mock, Shutdown::new());
        shard_service.register(
            SHARDS,
            &epochs([(0, 1)]),
            Some(Utc::now() + ChronoDuration::seconds(300)),
            ShardLeaseRevision(1),
        );

        let mut delays = Vec::new();
        for _ in 0..7 {
            delays.push(service.renew_shard_lease().await);
        }
        assert_eq!(
            delays,
            vec![1u64, 2, 4, 8, 16, 30, 30]
                .into_iter()
                .map(|secs| Some(Duration::from_secs(secs)))
                .collect::<Vec<_>>(),
            "the backoff doubles from 1 s and stops at the 30 s ceiling"
        );

        // Call 8 is granted: a 30 s lease.
        let cadence = service
            .renew_shard_lease()
            .await
            .expect("a granted lease has a cadence");
        assert!(
            cadence > Duration::from_secs(9) && cadence <= Duration::from_secs(10),
            "the cadence is one third of the granted TTL, got {cadence:?}"
        );

        let mut after_grant = Vec::new();
        for _ in 0..6 {
            after_grant.push(service.renew_shard_lease().await.unwrap());
        }
        assert_eq!(
            after_grant[0],
            Duration::from_secs(1),
            "a grant resets the backoff to its first step"
        );
        assert_eq!(after_grant[1], Duration::from_secs(2));
        assert_eq!(after_grant[2], Duration::from_secs(4));
        assert_eq!(after_grant[3], Duration::from_secs(8));
        assert!(
            after_grant[4] > Duration::from_secs(9) && after_grant[4] <= Duration::from_secs(10),
            "and the ceiling now follows the granted TTL / 3, got {:?}",
            after_grant[4]
        );
        assert_eq!(after_grant[5], after_grant[4], "where it stays");
    }
}
