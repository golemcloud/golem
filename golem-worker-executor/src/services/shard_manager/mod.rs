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

use crate::metrics::sharding::{
    ShardLeaseRenewalOutcome, record_shard_lease_metrics_started, record_shard_lease_renewal,
};
use crate::services::shard::ShardService;
use crate::services::shutdown::{self, Shutdown};
use async_trait::async_trait;
use futures::future::BoxFuture;
use golem_common::base_model::shard_lease;
use golem_common::model::{ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId};
use golem_service_base::clients::shard_manager::{
    ExecutorEndpoint, ShardLease, ShardLeaseError, ShardManagerError,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Floor on the derived renewal cadence, so a pathologically short lease cannot
/// turn the loop into a busy loop. Also the first step of the retry backoff.
const MIN_RENEWAL_INTERVAL: Duration = shard_lease::min_renewal_interval();

/// Absolute ceiling on the retry backoff, so a shard-manager
/// outage never becomes one RPC per second per executor. The effective ceiling
/// is the smaller of this and the cadence the last granted lease implied.
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the renewal loop waits before its next pass. `None` parks the loop:
/// the granted lease never expires, so there is nothing to renew and only the
/// shutdown arm can fire.
pub type RenewalDelay = Option<Duration>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenewalRetryState {
    backoff: Duration,
    granted_cadence: RenewalDelay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenewalRetryEvent {
    Failed,
    Granted(RenewalDelay),
}

/// What the receipt path did with the agents on a delivered set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Agents on lost shards were swept, and agents were recovered for the set.
    Recovered,
    /// Swept, but not recovered: the local lease had lapsed, and a fenced executor starts
    /// nothing. An accepted grant runs the recovery once the lease is live.
    DeferredUntilLeaseIsLive,
}

/// Fired when a re-registration or a corrected renewal replaces this executor's
/// shard assignment, so running agents are recovered for the new set exactly as
/// the initial registration and `assign_shards_internal` do. Installed by
/// `WorkerExecutorImpl::new`, which is the only place that can name `Ctx`.
pub type ShardAssignmentChangedHookFn =
    dyn Fn() -> BoxFuture<'static, Result<RecoveryOutcome, anyhow::Error>> + Send + Sync;
pub type ShardAssignmentChangedHook = Arc<ShardAssignmentChangedHookFn>;

/// Names one attempt at recovering agents for the set, handed out by
/// [`ShardManagerService::recovery_deferred`] and handed back by
/// [`ShardManagerService::recovery_succeeded`]. A recovery can only retire the
/// attempt it was started for, so one that ran to completion after a newer
/// delivery deferred its own cannot report that newer one done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryTicket(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryDebt {
    NoDebt,
    Owed(RecoveryTicket),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryDebtEvent {
    Deferred(RecoveryTicket),
    Completed(RecoveryTicket),
    LeaseAdopted(Option<ShardDeliveryOutcome>),
}

#[async_trait]
pub trait ShardManagerService: Send + Sync {
    /// Registers this executor and returns the shard assignment the manager
    /// granted it.
    async fn register(
        &self,
        endpoint: ExecutorEndpoint,
        pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError>;

    /// Graceful release of the shard lease. Never fails a shutdown.
    async fn deregister(&self);

    /// Requests a renewal before the timer fires. The grant identifies the manager
    /// process in charge and carries its set. No-op by default.
    fn renew_now(&self) {}

    /// Installs the hook fired when a re-registration or a corrected renewal
    /// replaces this executor's shard assignment. No-op by default: an
    /// implementation that never re-registers has nothing to announce.
    fn set_assignment_changed_hook(&self, _hook: &ShardAssignmentChangedHook) {}

    /// Records that recovery of the agents for the current set is owed and returns its ticket.
    /// This precedes the recovery attempt. A lapsed lease, a failed recovery and a running
    /// recovery all leave the debt owed. The next accepted grant runs recovery once the lease
    /// is live. A stale renewal can extend the lease but does not run recovery. No-op by default.
    fn recovery_deferred(&self) -> RecoveryTicket {
        RecoveryTicket(0)
    }

    /// Reports that the attempt started under `ticket` ran to completion. It retires the owed
    /// recovery only if nothing newer was registered since: a newer registration means a delivery
    /// this attempt did not see, whose agents it therefore did not start. No-op by default.
    fn recovery_succeeded(&self, _ticket: RecoveryTicket) {}
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

/// Coordinates the announcements made from the renewal loop, so a slow one cannot stall lease
/// renewal.
///
/// Giving up a still-loading agent waits for its whole component load and replay with no
/// timeout, so a set-changing renewal that awaited [`GrpcShardManagerService::announce_assignment_changed`]
/// inline would send no further renewal RPCs until that sweep finished, and the lease lapses for
/// the whole executor - the bug this exists to fix. At most one announcement runs at a time; a
/// request that arrives while one is already running is coalesced into exactly one more run after
/// it finishes, so nothing requested is ever dropped, and a loop that keeps correcting the set
/// does not pile up concurrent sweeps racing each other over the same agents.
///
/// Deliberately not tracked by [`Shutdown::spawn`]: a sweep can run for as long as the slowest
/// agent's replay, and the renewal loop's own deregister - which *is* tracked, so `main` waits for
/// it - has to still land inside the shutdown grace regardless. Tracking this task the same way
/// would let a stale sweep hold the process open past that grace for a recovery nobody is waiting
/// on any more; a bare `tokio::spawn` is simply cut off at its next await point when the runtime
/// is dropped, which is the right fate for best-effort work like this one.
struct AnnouncementSingleFlight {
    state: AtomicU8,
}

impl AnnouncementSingleFlight {
    /// No task running.
    const IDLE: u8 = 0;
    /// A task is running the announcement.
    const RUNNING: u8 = 1;
    /// A task is running, and a request arrived while it did; it reruns once more before going
    /// idle.
    const RUNNING_PENDING: u8 = 2;

    fn new() -> Self {
        Self {
            state: AtomicU8::new(Self::IDLE),
        }
    }

    /// Requests one announcement, without waiting for it to run.
    ///
    /// Spawns the task that runs it only when none is already running; a request that arrives
    /// mid-run instead marks the running task to loop once more, so a burst of requests while a
    /// sweep is in flight still produces at most one extra run after it.
    fn request(svc: &Arc<GrpcShardManagerService>) {
        loop {
            let state = &svc.announcement_single_flight.state;
            match state.compare_exchange(
                Self::IDLE,
                Self::RUNNING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    let svc = svc.clone();
                    tokio::spawn(Self::run(svc));
                    return;
                }
                Err(Self::RUNNING) => {
                    match state.compare_exchange(
                        Self::RUNNING,
                        Self::RUNNING_PENDING,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => return,
                        // The running task raced this and just went idle (or another requester
                        // already coalesced): re-read the state and try again from scratch.
                        Err(_) => continue,
                    }
                }
                // Already coalesced: a rerun is guaranteed without this request doing anything.
                Err(_) => return,
            }
        }
    }

    /// The task body [`Self::request`] spawns: runs the announcement, then either goes idle or,
    /// if a request was coalesced while it ran, runs it once more.
    async fn run(svc: Arc<GrpcShardManagerService>) {
        loop {
            svc.announce_assignment_changed().await;
            let state = &svc.announcement_single_flight.state;
            match state.compare_exchange(
                Self::RUNNING,
                Self::IDLE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(Self::RUNNING_PENDING) => {
                    state.store(Self::RUNNING, Ordering::SeqCst);
                    continue;
                }
                Err(other) => unreachable!(
                    "AnnouncementSingleFlight in state {other}, which only `request` and `run` \
                     touch and neither ever stores it"
                ),
            }
        }
    }
}

pub struct GrpcShardManagerService {
    client: Arc<dyn golem_service_base::clients::shard_manager::ShardLeaseClient>,
    shard_service: Arc<dyn ShardService>,
    shutdown: Shutdown,
    /// The identity of this executor's shard lease. Regenerated whenever the
    /// manager answers `LeaseNotFound`, because from the manager's point of
    /// view this process is then a new instance at the same address.
    executor_id: RwLock<Uuid>,
    /// The registration arguments, kept so a re-register after `LeaseNotFound`
    /// can repeat it without going back through `WorkerExecutorImpl`.
    registration: RwLock<Option<(ExecutorEndpoint, Option<String>)>>,
    /// Weak self-reference, so `register` can spawn the renewal loop without
    /// the loop keeping this service alive.
    me: Weak<Self>,
    renewal_loop_started: AtomicBool,
    /// Wakes the renewal loop ahead of its timer; see [`ShardManagerService::renew_now`].
    renew_now: Arc<tokio::sync::Notify>,
    /// Exponential backoff for failed renewals. It starts at `MIN_RENEWAL_INTERVAL`
    /// and doubles up to `MAX_RETRY_INTERVAL`, capped by the granted cadence when present.
    /// Every grant resets it.
    retry: RwLock<RenewalRetryState>,
    /// Fired when a re-registration or a corrected renewal replaces the shard assignment.
    ///
    /// Weak, following `LazyWorkerActivator`: the hook closes over the service graph that owns
    /// this service, so a strong reference here would be a cycle nothing could free.
    /// `WorkerExecutorImpl` holds the strong one, so the hook lives as long as the executor.
    assignment_changed_hook: RwLock<Option<Weak<ShardAssignmentChangedHookFn>>>,
    /// Shortest per-attempt deadline this executor will use; see [`Self::rpc_deadline`].
    /// A field rather than the constant so a test does not have to wait out the production floor.
    rpc_deadline_floor: Duration,
    /// Atomic encoding of `RecoveryDebt`. Only `recovery_debt_transition` decides changes.
    /// An unchanged accepted grant still requests recovery while debt is owed. Completion
    /// retires only its own ticket, so an older attempt cannot retire newer debt.
    recovery_debt: AtomicU64,
    /// Source of tickets; never reused, so an attempt that outlives a full clear-and-defer cycle
    /// cannot collide with a later one.
    recovery_tickets: AtomicU64,
    /// Coordinates announcements requested from the renewal loop; see
    /// [`AnnouncementSingleFlight`].
    announcement_single_flight: AnnouncementSingleFlight,
}

impl GrpcShardManagerService {
    pub fn new(
        client: Arc<dyn golem_service_base::clients::shard_manager::ShardLeaseClient>,
        shard_service: Arc<dyn ShardService>,
        shutdown: Shutdown,
    ) -> Arc<Self> {
        Self::new_with_rpc_deadline_floor(
            client,
            shard_service,
            shutdown,
            shard_lease::rpc_deadline_floor(),
        )
    }

    /// [`Self::new`] with the per-attempt deadline floor taken as a parameter, so a test does not
    /// have to wait out the production one.
    pub fn new_with_rpc_deadline_floor(
        client: Arc<dyn golem_service_base::clients::shard_manager::ShardLeaseClient>,
        shard_service: Arc<dyn ShardService>,
        shutdown: Shutdown,
        rpc_deadline_floor: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            client,
            shard_service,
            shutdown,
            executor_id: RwLock::new(Uuid::new_v4()),
            registration: RwLock::new(None),
            me: me.clone(),
            renewal_loop_started: AtomicBool::new(false),
            renew_now: Arc::new(tokio::sync::Notify::new()),
            retry: RwLock::new(RenewalRetryState {
                backoff: MIN_RENEWAL_INTERVAL,
                granted_cadence: None,
            }),
            assignment_changed_hook: RwLock::new(None),
            rpc_deadline_floor,
            recovery_debt: AtomicU64::new(encode_recovery_debt(RecoveryDebt::NoDebt)),
            recovery_tickets: AtomicU64::new(0),
            announcement_single_flight: AnnouncementSingleFlight::new(),
        })
    }

    /// How long one lease RPC may take: half the cadence the last grant implied, so a manager
    /// that accepts the call and never answers costs half the gap to the next renewal rather than
    /// all of it - and never less than the floor, so it always outlasts the manager's own budget
    /// for storing what the call asked for.
    ///
    /// The retry backoff caps the wait between attempts. This deadline limits one attempt.
    /// Using the backoff cap here would let a hung call consume a whole renewal period.
    fn rpc_deadline(&self) -> Duration {
        shard_lease::rpc_deadline(
            self.retry.read().unwrap().granted_cadence,
            self.rpc_deadline_floor,
        )
    }

    fn executor_id(&self) -> Uuid {
        *self.executor_id.read().unwrap()
    }

    fn apply_retry_event(&self, event: RenewalRetryEvent) -> RenewalDelay {
        let mut state = self.retry.write().unwrap();
        let (next, delay) = renewal_retry_transition(*state, event);
        *state = next;
        delay
    }

    fn apply_recovery_event(&self, event: RecoveryDebtEvent) -> bool {
        // `try_update` repeats the transition with the current state after a failed CAS.
        let previous = self
            .recovery_debt
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |encoded| {
                let (next, _) = recovery_debt_transition(decode_recovery_debt(encoded), event);
                Some(encode_recovery_debt(next))
            })
            .expect("every recovery event supplies a state");
        recovery_debt_transition(decode_recovery_debt(previous), event).1
    }

    /// Tell the executor its shard assignment changed, so running
    /// agents are recovered for the new set. A failure here is logged, never
    /// fatal — the lease itself is already installed — and remembered, so the
    /// next accepted grant runs the recovery again once the lease is live.
    ///
    /// The attempt is registered here as well as inside the hook: the hook is
    /// the receipt path, which registers and retires its own ticket, but a
    /// hook that is missing or that is a test double has no ticket of its own,
    /// and this one is retired only on a completed recovery.
    async fn announce_assignment_changed(&self) {
        let ticket = self.recovery_deferred();
        let hook = self
            .assignment_changed_hook
            .read()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade);
        let Some(hook) = hook else {
            // Only reachable once the executor that owns the hook is gone, i.e. during teardown.
            // Left owed rather than passed over: a set-changing grant has been applied and the
            // agents it moved have not been swept.
            warn!("No assignment-changed hook is installed; agents were not recovered");
            return;
        };
        match hook().await {
            Ok(RecoveryOutcome::Recovered) => self.recovery_succeeded(ticket),
            Ok(RecoveryOutcome::DeferredUntilLeaseIsLive) => info!(
                "Recovering agents for the shard set waits for the lease; running it on the next grant"
            ),
            Err(error) => {
                warn!(%error, "Recovering agents for the shard set failed; retrying on the next grant")
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
        let renew_now = self.renew_now.clone();
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
                    _ = renew_now.notified() => {}
                }
                let svc = match svc_weak.upgrade() {
                    Some(svc) => svc,
                    None => {
                        info!("ShardManagerService was dropped, stopping renewal loop");
                        break;
                    }
                };
                // Raced against the token as well, not just the sleep before it: a termination
                // signal arriving mid-renewal has to be seen inside the grace `main` waits, which
                // is the window the deregister below must be sent in. Abandoning the renewal
                // costs nothing: the process is stopping and handing the lease back.
                //
                // The renewal itself never runs the announcement inline any more - see
                // `renew_shard_lease_internal` and `AnnouncementSingleFlight` - specifically so
                // that an unbounded sweep following a set change or a re-registration can never
                // be what the token is raced against here: this arm always returns promptly, and
                // the loop keeps renewing while a sweep runs in its own task.
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        svc.deregister().await;
                        break;
                    }
                    (delay, announcement_owed) = svc.renew_shard_lease_internal() => {
                        renewal_delay = delay;
                        if announcement_owed {
                            AnnouncementSingleFlight::request(&svc);
                        }
                    }
                }
            }
        });
    }

    /// Applies a granted lease and returns the cadence for the next pass and whether recovery
    /// is owed. The renewal loop schedules recovery through [`AnnouncementSingleFlight`], so a
    /// slow sweep does not delay the next renewal RPC.
    ///
    /// An accepted renewal records the expiry, revision and executor count,
    /// even when the shard set does not change. After an accepted reply, the
    /// renewal loop requests recovery if the set changed or recovery is still owed.
    ///
    /// A grant whose set is older than the last delivery applied crossed with a
    /// push on the network: its set is ignored, or it would put the older set back,
    /// and its lease is adopted, because it answers this executor's own request.
    fn adopt_lease(&self, lease: ShardLease) -> (RenewalDelay, bool) {
        let ShardLease {
            shard_epochs,
            expires_at,
            revision,
            executor_count,
        } = lease;
        let shard_epochs: HashMap<ShardId, ShardEpoch> = shard_epochs.into_iter().collect();
        let outcome =
            self.shard_service
                .update_lease(&shard_epochs, expires_at, revision, executor_count);
        let announcement_owed = self.apply_recovery_event(RecoveryDebtEvent::LeaseAdopted(
            outcome.as_ref().ok().copied(),
        ));
        match outcome {
            Ok(ShardDeliveryOutcome::Applied { set_changed: true }) => {
                info!(
                    %revision,
                    "Shard lease renewal corrected the shard set; sweeping and recovering agents"
                );
            }
            Ok(ShardDeliveryOutcome::Applied { set_changed: false }) => {
                if announcement_owed {
                    info!(%revision, "Running the agent recovery still owed from an earlier delivery");
                }
            }
            Ok(ShardDeliveryOutcome::Stale { delivered, applied }) => {
                warn!(
                    %delivered,
                    %applied,
                    "Ignoring the shard set of a renewal older than the last delivery applied; the lease clock moved"
                );
            }
            // Only a push is refused for its sender: a reply's sender is the one followed.
            Ok(ShardDeliveryOutcome::FromAnotherManager { .. }) => {}
            Err(error) => {
                warn!(%error, "Failed to apply a renewed shard lease");
            }
        };
        let cadence = renewal_interval_for(Some(expires_at), Instant::now());
        self.apply_retry_event(RenewalRetryEvent::Granted(cadence));
        (cadence, announcement_owed)
    }

    /// [`ShardManagerService::register`], returning the grant itself: the number of shards and the
    /// lease, with the revision it was delivered at.
    async fn register_granted(
        &self,
        endpoint: ExecutorEndpoint,
        pod_name: Option<String>,
    ) -> Result<(usize, ShardLease), ShardManagerError> {
        *self.registration.write().unwrap() = Some((endpoint, pod_name.clone()));

        let registration = self
            .client
            .register(endpoint, pod_name, self.executor_id())
            .await?;

        let number_of_shards: usize = registration.number_of_shards.try_into().map_err(|_| {
            ShardManagerError::ConversionError(format!(
                "RegisterSuccess.number_of_shards {} does not fit a usize",
                registration.number_of_shards
            ))
        })?;

        // `ShardId::from_agent_id` divides by it.
        if number_of_shards == 0 {
            return Err(ShardManagerError::ConversionError(
                "RegisterSuccess.number_of_shards must not be 0".to_string(),
            ));
        }

        // Started here rather than by the caller because this is the first
        // point at which a lease exists, and started unconditionally so that a
        // graceful shutdown always deregisters (a termination signal trips the
        // same token). A grant with no expiry parks the loop on its shutdown arm
        // and issues no RPCs at all.
        let cadence = renewal_interval_for(Some(registration.lease.expires_at), Instant::now());
        record_shard_lease_metrics_started();
        self.apply_retry_event(RenewalRetryEvent::Granted(cadence));
        self.start_renewal_loop(cadence);

        Ok((number_of_shards, registration.lease))
    }

    /// Renews and adopts one grant, including re-registration after a lost lease. The one
    /// renewal loop awaits this pass before it starts another. Recovery is returned as a
    /// separate obligation and does not delay renewal.
    async fn renew_shard_lease_internal(&self) -> (RenewalDelay, bool) {
        let held = match self.shard_service.current_assignment() {
            Ok(assignment) => assignment.held_epochs(),
            Err(error) => {
                record_shard_lease_renewal(ShardLeaseRenewalOutcome::NotRegistered);
                warn!(%error, "Skipping shard lease renewal, no shard assignment yet");
                return (self.apply_retry_event(RenewalRetryEvent::Failed), false);
            }
        };

        let executor_id = self.executor_id();
        let deadline = self.rpc_deadline();
        let renewed =
            match tokio::time::timeout(deadline, self.client.renew_shard_lease(executor_id, held))
                .await
            {
                Ok(renewed) => renewed,
                Err(_elapsed) => {
                    record_shard_lease_renewal(ShardLeaseRenewalOutcome::Timeout);
                    warn!(
                        deadline_ms = deadline.as_millis(),
                        "Shard lease renewal did not answer in time; backing off"
                    );
                    return (self.apply_retry_event(RenewalRetryEvent::Failed), false);
                }
            };
        match renewed {
            Ok(lease) => {
                record_shard_lease_renewal(ShardLeaseRenewalOutcome::Granted);
                self.adopt_lease(lease)
            }
            Err(ShardLeaseError::LeaseNotFound(details)) => {
                record_shard_lease_renewal(ShardLeaseRenewalOutcome::LeaseNotFound);
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
                        (self.apply_retry_event(RenewalRetryEvent::Failed), false)
                    }
                    // Bounded like a renewal: the assignment is already cleared, so a manager that
                    // accepts the call and stalls would otherwise hold this executor with no shards
                    // and no registration for as long as it stalls.
                    Some((endpoint, pod_name)) => match tokio::time::timeout(
                        self.rpc_deadline(),
                        self.register_granted(endpoint, pod_name),
                    )
                    .await
                    {
                        Err(_) => {
                            warn!(
                                deadline_ms = self.rpc_deadline().as_millis(),
                                "Re-registration after a lost lease timed out, retrying"
                            );
                            (self.apply_retry_event(RenewalRetryEvent::Failed), false)
                        }
                        Ok(Ok((number_of_shards, lease))) => {
                            let outcome = self.shard_service.register(
                                number_of_shards,
                                &lease.shard_epochs.into_iter().collect(),
                                Some(lease.expires_at),
                                lease.revision,
                                lease.executor_count,
                            );
                            if let ShardDeliveryOutcome::Stale { delivered, applied } = outcome {
                                // A push read from a later state of the same process crossed
                                // this reply and was applied first; its set is the newer one.
                                warn!(
                                    %delivered,
                                    %applied,
                                    "The re-registration's shard set is older than the last delivery applied and was ignored"
                                );
                            }
                            info!(
                                executor_id = %fresh_executor_id,
                                "Re-registered with the shard manager after a lost lease"
                            );
                            // The same announcement the initial
                            // registration and `assign_shards_internal` make - owed to the
                            // caller, exactly like a set-changing grant.
                            let delay =
                                renewal_interval_for(Some(lease.expires_at), Instant::now());
                            (delay, true)
                        }
                        Ok(Err(error)) => {
                            warn!(%error, "Re-registration after a lost lease failed");
                            (self.apply_retry_event(RenewalRetryEvent::Failed), false)
                        }
                    },
                }
            }
            Err(error) => {
                // Transport class: nothing local changes, so the stored expiry
                // runs down on its own and the self-fence starts refusing
                // admission the moment it passes.
                record_shard_lease_renewal(ShardLeaseRenewalOutcome::Failed);
                warn!(%error, "Shard lease renewal failed, retrying");
                (self.apply_retry_event(RenewalRetryEvent::Failed), false)
            }
        }
    }
}

/// Gives the retry state and delay after a failure or grant. A grant resets the backoff.
fn renewal_retry_transition(
    state: RenewalRetryState,
    event: RenewalRetryEvent,
) -> (RenewalRetryState, RenewalDelay) {
    match event {
        RenewalRetryEvent::Failed => {
            let cap = state
                .granted_cadence
                .unwrap_or(MAX_RETRY_INTERVAL)
                .min(MAX_RETRY_INTERVAL);
            let delay = state.backoff.min(cap);
            (
                RenewalRetryState {
                    backoff: delay.saturating_mul(2).min(cap),
                    ..state
                },
                Some(delay),
            )
        }
        RenewalRetryEvent::Granted(cadence) => (
            RenewalRetryState {
                backoff: MIN_RENEWAL_INTERVAL,
                granted_cadence: cadence,
            },
            cadence,
        ),
    }
}

/// Gives the debt after the event and whether the grant needs a recovery announcement.
/// Adoption preserves debt. Only completion of the current ticket clears it.
fn recovery_debt_transition(debt: RecoveryDebt, event: RecoveryDebtEvent) -> (RecoveryDebt, bool) {
    match event {
        RecoveryDebtEvent::Deferred(ticket) => {
            let next = match debt {
                RecoveryDebt::Owed(current) if current.0 > ticket.0 => debt,
                _ => RecoveryDebt::Owed(ticket),
            };
            (next, false)
        }
        RecoveryDebtEvent::Completed(ticket) => (
            if debt == RecoveryDebt::Owed(ticket) {
                RecoveryDebt::NoDebt
            } else {
                debt
            },
            false,
        ),
        RecoveryDebtEvent::LeaseAdopted(outcome) => (
            debt,
            match outcome {
                Some(ShardDeliveryOutcome::Applied { set_changed }) => {
                    set_changed || matches!(debt, RecoveryDebt::Owed(_))
                }
                _ => false,
            },
        ),
    }
}

fn decode_recovery_debt(encoded: u64) -> RecoveryDebt {
    match encoded {
        0 => RecoveryDebt::NoDebt,
        ticket => RecoveryDebt::Owed(RecoveryTicket(ticket)),
    }
}

fn encode_recovery_debt(debt: RecoveryDebt) -> u64 {
    match debt {
        RecoveryDebt::NoDebt => 0,
        RecoveryDebt::Owed(ticket) => ticket.0,
    }
}

/// `(expires_at - now) / 3`, floored, so three attempts fit inside one lease.
///
/// A lease that never expires yields `None`, which parks the
/// renewal loop instead of polling it — there is nothing to renew, and a
/// polling loop would be one wasted RPC per second per executor.
fn renewal_interval_for(expires_at: Option<Instant>, now: Instant) -> RenewalDelay {
    let expires_at = expires_at?;
    Some(shard_lease::renewal_interval(
        expires_at.saturating_duration_since(now),
    ))
}

#[async_trait]
#[golem_async_trait::region_simple]
impl ShardManagerService for GrpcShardManagerService {
    fn renew_now(&self) {
        self.renew_now.notify_one();
    }

    async fn register(
        &self,
        endpoint: ExecutorEndpoint,
        pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError> {
        let (number_of_shards, lease) = self.register_granted(endpoint, pod_name).await?;
        Ok(ShardAssignment {
            number_of_shards,
            executor_count: lease.executor_count.get(),
            shard_epochs: lease.shard_epochs.into_iter().collect(),
            expires_at: Some(lease.expires_at),
            revision: Some(lease.revision),
        })
    }

    async fn deregister(&self) {
        let held = self
            .shard_service
            .try_get_current_assignment()
            .map(|assignment| assignment.held_epochs())
            .unwrap_or_default();

        let executor_id = self.executor_id();
        match tokio::time::timeout(
            shutdown::DEREGISTER_DEADLINE,
            self.client.deregister(executor_id, held),
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

    fn recovery_deferred(&self) -> RecoveryTicket {
        let ticket = RecoveryTicket(self.recovery_tickets.fetch_add(1, Ordering::SeqCst) + 1);
        self.apply_recovery_event(RecoveryDebtEvent::Deferred(ticket));
        ticket
    }

    fn recovery_succeeded(&self, ticket: RecoveryTicket) {
        self.apply_recovery_event(RecoveryDebtEvent::Completed(ticket));
    }
}

/// Single-shard implementation for local development. Returns a single shard
/// assignment without contacting a real shard manager.
pub struct ShardManagerServiceSingleShard;

#[async_trait]
#[golem_async_trait::region_simple]
impl ShardManagerService for ShardManagerServiceSingleShard {
    async fn register(
        &self,
        _endpoint: ExecutorEndpoint,
        _pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError> {
        // `expires_at: None` — the single binary has no shard manager to renew
        // against and must never fence itself.
        Ok(ShardAssignment::unexpiring(1, [ShardId::new(0)]))
    }

    async fn deregister(&self) {}
}

#[cfg(test)]
impl GrpcShardManagerService {
    /// Runs one production renewal pass and waits for its recovery before assertions run.
    async fn renew_shard_lease(&self) -> RenewalDelay {
        let (delay, announcement_owed) = self.renew_shard_lease_internal().await;
        if announcement_owed {
            self.announce_assignment_changed().await;
        }
        delay
    }
}

#[cfg(test)]
mod tests;
