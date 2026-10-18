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

//! Leadership campaign for distributed (etcd) mode: elects one replica, keeps its lease alive,
//! and produces the [`LeaderFence`] that makes leadership a precondition of every state write.

use crate::config::EtcdConfig;
use crate::metrics;
use crate::sharding::election_watch::{CampaignView, Campaigner, ElectionWatch, ElectionWatcher};
use crate::sharding::error::ShardManagerError;
use crate::sharding::etcd_connection::connect_for_election;
use crate::sharding::etcd_retry::retry_retriable_until;
use etcd_client::{
    Client, Compare, CompareOp, LeaderKey, LeaseClient, LeaseKeepAliveStream, LeaseKeeper,
    ProclaimOptions, TxnOp, TxnOpResponse, TxnResponse,
};
use golem_common::retriable_error::IsRetriableError;
use golem_service_base::clients::shard_manager_leader::LeaderAddress;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep, sleep_until, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Election name for the shard manager's leadership.
pub const LEADER_ELECTION_NAME: &str = "/golem/shard-manager/leader";

/// How often a standby logs who currently holds the leadership, so that a replica blocked in its
/// campaign is legible in the log rather than looking hung.
const STANDBY_LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Proof that a specific campaign was won, in the shape etcd's transaction compare wants. Every
/// state write carries it, so a replica that lost the leadership cannot overwrite its successor.
#[derive(Clone, Debug)]
pub struct LeaderFence {
    key: Vec<u8>,
    create_revision: i64,
}

impl LeaderFence {
    fn from_leader_key(leader: &LeaderKey) -> Result<Self, ShardManagerError> {
        let key = leader.key().to_vec();
        if key.is_empty() {
            return Err(ShardManagerError::Internal(
                "etcd campaign returned an empty leader key".to_string(),
            ));
        }

        let create_revision = leader.rev();
        if create_revision < 1 {
            return Err(ShardManagerError::Internal(format!(
                "etcd campaign returned leader key creation revision {create_revision}, which is \
                 reserved for absent keys"
            )));
        }

        Ok(Self {
            key,
            create_revision,
        })
    }

    #[doc(hidden)]
    pub fn for_test(key: impl Into<Vec<u8>>, create_revision: i64) -> Self {
        Self {
            key: key.into(),
            create_revision,
        }
    }

    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn key_str(&self) -> String {
        String::from_utf8_lossy(&self.key).into_owned()
    }

    pub fn create_revision(&self) -> i64 {
        self.create_revision
    }

    /// The compare that makes leadership a precondition of a transaction.
    pub fn compare(&self) -> Compare {
        Compare::create_revision(self.key.clone(), CompareOp::Equal, self.create_revision)
    }

    /// The operation a fenced transaction's else-branch starts with, so that a refusal can be
    /// attributed by [`Self::held_after_refusal`].
    pub fn read_back(&self) -> TxnOp {
        TxnOp::get(self.key.clone(), None)
    }

    /// Whether a transaction carrying [`Self::compare`] was refused while this fence still held,
    /// judged from its else-branch, which must start with [`Self::read_back`]. If it did, some
    /// other precondition failed. Leadership that cannot be confirmed - a missing or recreated
    /// leader key, or no else-response at all - is reported as not held, which is the safe answer.
    pub fn held_after_refusal(&self, response: &TxnResponse) -> bool {
        matches!(
            response.op_responses().first(),
            Some(TxnOpResponse::Get(get))
                if get.kvs().first().is_some_and(|kv| {
                    kv.create_revision() == self.create_revision
                })
        )
    }
}

/// Why a leadership lease stopped being held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseLossReason {
    KeepAliveRequestFailed,
    KeepAliveStreamFailed,
    KeepAliveStreamClosed,
    LeaseExpired,
    UnexpectedLeaseId,
    RenewalDeadlineExceeded,
    LeaderKeyGone,
}

impl std::fmt::Display for LeaseLossReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::KeepAliveRequestFailed => "the keepalive request could not be sent",
            Self::KeepAliveStreamFailed => "the keepalive stream failed",
            Self::KeepAliveStreamClosed => "etcd closed the keepalive stream",
            Self::LeaseExpired => "etcd reported the lease as expired",
            Self::UnexpectedLeaseId => "etcd answered for a different lease",
            Self::RenewalDeadlineExceeded => "no renewal was acknowledged within the lease TTL",
            Self::LeaderKeyGone => "the won leader key was already gone",
        };
        f.write_str(text)
    }
}

/// The leadership lease was lost. There is no success counterpart, on purpose: see
/// [`LeaseKeepAlive::run`].
#[derive(Debug, thiserror::Error)]
#[error("the etcd leader lease {lease_id:#x} was lost: {reason}")]
pub struct LeaseLost {
    pub lease_id: i64,
    pub reason: LeaseLossReason,
    #[source]
    pub source: Option<etcd_client::Error>,
}

/// Until when this replica may believe that it holds its leadership lease: one TTL after the send
/// of the last renewal that etcd acknowledged. The keepalive moves it; the gate serves only before
/// it, so a leader that was paused past its lease (a long GC pause, CPU throttling, a suspended
/// virtual machine) serves nothing after it resumes, even before its keepalive task runs and stops
/// the process.
///
/// The deadline is kept on [`belief_clock`], not on the monotonic clock of the runtime: on Linux
/// the monotonic clock stands still while a machine is suspended, and a leader that woke up would
/// then still count the time it slept as time left on a lease that etcd expired long ago.
#[derive(Clone, Debug)]
pub struct LeaseBelief {
    deadline: Arc<std::sync::Mutex<Duration>>,
}

impl LeaseBelief {
    fn new(deadline: Duration) -> Self {
        Self {
            deadline: Arc::new(std::sync::Mutex::new(deadline)),
        }
    }

    fn set(&self, deadline: Duration) {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) = deadline;
    }

    pub fn held(&self) -> bool {
        self.held_at(belief_clock::now())
    }

    /// Moves the deadline to `remaining` from now. **Tests only.**
    #[cfg(test)]
    pub(crate) fn rearm_for(&self, remaining: Duration) {
        self.set(belief_clock::now() + remaining);
    }

    /// How long the belief still holds; zero once it has lapsed.
    pub(crate) fn remaining(&self) -> Duration {
        self.deadline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .saturating_sub(belief_clock::now())
    }

    /// Whether the belief holds at `now` on [`belief_clock`].
    fn held_at(&self, now: Duration) -> bool {
        now < *self.deadline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A belief that ends at `deadline`. **Tests only.**
    #[doc(hidden)]
    pub fn until(deadline: Instant) -> Self {
        Self::new(belief_clock::now() + deadline.saturating_duration_since(Instant::now()))
    }
}

/// The moment a request was sent, on both clocks. The lease etcd holds started when the request
/// reached it, so the belief is dated from the send: read on the belief clock at the
/// acknowledgement, a suspend between the send and the answer would not be counted. The watchdog
/// still sleeps until the monotonic time.
#[derive(Clone, Copy, Debug)]
struct SentAt {
    monotonic: Instant,
    belief: Duration,
}

impl SentAt {
    fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            belief: belief_clock::now(),
        }
    }
}

/// The clock that the lease belief is measured on. On Linux it is `CLOCK_BOOTTIME`, which counts
/// the time a machine spends suspended; elsewhere the monotonic clock, which is the best the
/// platform offers. The watchdog still sleeps on the runtime's monotonic clock: that only decides
/// when the keepalive task wakes, and the gate's check does not depend on it.
mod belief_clock {
    use std::time::Duration;

    #[cfg(target_os = "linux")]
    pub(super) fn now() -> Duration {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `time` is a valid timespec that outlives the call, and CLOCK_BOOTTIME exists on
        // every Linux that Golem runs on.
        let result = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) };
        assert_eq!(result, 0, "CLOCK_BOOTTIME cannot be read");
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn now() -> Duration {
        use std::sync::OnceLock;
        use std::time::Instant;

        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed()
    }
}

/// Bounds how long this replica may believe it holds the lease: a black-holed connection yields
/// neither an error nor a close, so silence has to become a bounded detection. Kept apart from
/// [`LeaseKeepAlive`] so the anchoring rule can be tested without a live stream.
struct RenewalWatchdog {
    ttl: Duration,
    /// When this replica stops believing it holds the lease: one TTL after the *send* of the last
    /// renewal etcd acknowledged, since that is when etcd's own clock on the lease restarted.
    deadline: Instant,
    /// Send times of renewals not yet acknowledged, oldest first. etcd answers a stream in order,
    /// and the send is the earlier of the two instants, so a watchdog that errs gives the lease up
    /// sooner.
    unacknowledged: VecDeque<SentAt>,
    /// The same deadline, shared with the gate.
    belief: LeaseBelief,
}

impl RenewalWatchdog {
    /// Starts the belief window at `sent_at`: the instant the handshake was sent, which etcd has
    /// since answered.
    fn armed_at(sent_at: SentAt, ttl: Duration) -> Self {
        Self {
            ttl,
            deadline: sent_at.monotonic + ttl,
            unacknowledged: VecDeque::new(),
            belief: LeaseBelief::new(sent_at.belief + ttl),
        }
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn record_send(&mut self, sent_at: SentAt) {
        self.unacknowledged.push_back(sent_at);
    }

    /// Re-arms from the send this response answers, never from when it was read:
    /// [`LeaseKeepAlive::drive`] is entered more than once, and an entry that is dropped leaves its
    /// acknowledgement unread until the next one. A response with no send behind it is the
    /// handshake's own, which already armed the deadline.
    fn record_acknowledgement(&mut self) {
        if let Some(sent_at) = self.unacknowledged.pop_front() {
            self.deadline = sent_at.monotonic + self.ttl;
            self.belief.set(sent_at.belief + self.ttl);
        }
    }

    /// Drops the sends still outstanding on a replaced stream: nothing sent on it can be answered
    /// by its successor.
    fn forget_unacknowledged(&mut self) {
        self.unacknowledged.clear();
    }

    /// Re-arms from a renewal acknowledged outside the response stream - the one `keep_alive`
    /// performs when a reconnect succeeds.
    fn rearm_at(&mut self, sent_at: SentAt) {
        self.deadline = sent_at.monotonic + self.ttl;
        self.belief.set(sent_at.belief + self.ttl);
    }
}

/// Renews the leadership lease until it is lost.
pub struct LeaseKeepAlive {
    lease_id: i64,
    keeper: LeaseKeeper,
    stream: LeaseKeepAliveStream,
    watchdog: RenewalWatchdog,
    /// Re-establishes the keepalive when its stream breaks. The same client the leadership handle
    /// revokes with.
    lease_client: LeaseClient,
    request_timeout: Duration,
}

impl LeaseKeepAlive {
    /// Shared with the gate; see [`LeaseBelief`].
    pub fn belief(&self) -> LeaseBelief {
        self.watchdog.belief.clone()
    }

    /// A third of the TTL, as etcd's own client does, with a floor so a short test TTL is not a
    /// busy loop. Public so the failover test's bound is derived from the same formula.
    pub fn renewal_interval(ttl: Duration) -> Duration {
        std::cmp::max(ttl / 3, Duration::from_millis(200))
    }

    /// Renews until the lease is lost, and reports why. There is no success case on purpose: a
    /// keepalive that could stop cleanly would leave a leader serving a table it can no longer
    /// write.
    pub async fn run(mut self) -> LeaseLost {
        self.drive().await
    }

    async fn drive(&mut self) -> LeaseLost {
        let mut renew = interval(Self::renewal_interval(self.watchdog.ttl));
        renew.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            // Ordered: after a stall of one TTL the first tick and the deadline are ready together,
            // and an unordered select would declare the lease lost without ever having tried to
            // renew it.
            let broken = tokio::select! {
                biased;
                _ = renew.tick() => {
                    let sent_at = SentAt::now();
                    match self.keeper.keep_alive().await {
                        Ok(()) => {
                            self.watchdog.record_send(sent_at);
                            continue;
                        }
                        Err(err) => (LeaseLossReason::KeepAliveRequestFailed, Some(err)),
                    }
                }
                message = self.stream.message() => match message {
                    Ok(Some(response)) if response.id() != self.lease_id => {
                        return self.lost(LeaseLossReason::UnexpectedLeaseId, None);
                    }
                    Ok(Some(response)) if response.ttl() > 0 => {
                        self.watchdog.record_acknowledgement();
                        continue;
                    }
                    Ok(Some(_)) => return self.lost(LeaseLossReason::LeaseExpired, None),
                    Ok(None) => (LeaseLossReason::KeepAliveStreamClosed, None),
                    Err(err) => (LeaseLossReason::KeepAliveStreamFailed, Some(err)),
                },
                _ = sleep_until(self.watchdog.deadline) => {
                    return self.lost(LeaseLossReason::RenewalDeadlineExceeded, None);
                }
            };

            // A broken transport is not a lost lease: etcd still holds it across a member restart
            // or a raft leader change. Awaited here rather than in a select arm so the watchdog is
            // not starved; the deadline bounds it.
            let (reason, source) = broken;
            if let Err(expired) = self.reconnect(reason, source).await {
                return self.lost(expired, None);
            }
        }
    }

    /// Replaces a broken keeper and stream, retrying until the watchdog deadline, which is the only
    /// bound. A reconnect that succeeds has renewed the lease, so it re-arms the watchdog from its
    /// own send, as any acknowledged renewal does.
    async fn reconnect(
        &mut self,
        reason: LeaseLossReason,
        source: Option<etcd_client::Error>,
    ) -> Result<(), LeaseLossReason> {
        warn!(
            lease_id = self.lease_id,
            reason = %reason,
            error = source.map(|err| err.to_string()),
            "Re-establishing the leadership lease keepalive"
        );

        loop {
            let remaining = self.watchdog.remaining();
            if remaining.is_zero() {
                return Err(LeaseLossReason::RenewalDeadlineExceeded);
            }

            let attempt = std::cmp::min(self.request_timeout, remaining);
            let attempt_sent = SentAt::now();
            match timeout(attempt, self.lease_client.keep_alive(self.lease_id)).await {
                Ok(Ok((keeper, stream))) => {
                    self.keeper = keeper;
                    self.stream = stream;
                    self.watchdog.forget_unacknowledged();
                    self.watchdog.rearm_at(attempt_sent);
                    return Ok(());
                }
                Ok(Err(err)) if is_lease_gone(&err) => return Err(LeaseLossReason::LeaseExpired),
                Ok(Err(err)) => warn!(
                    lease_id = self.lease_id,
                    error = %err,
                    "A keepalive reconnect attempt failed"
                ),
                Err(_) => warn!(
                    lease_id = self.lease_id,
                    attempt = ?attempt,
                    "A keepalive reconnect attempt timed out"
                ),
            }

            // A refused connection returns instantly; pacing keeps the retry from spinning, and
            // never sleeping past the deadline keeps it from delaying the loss report.
            let pace = std::cmp::min(
                Self::renewal_interval(self.watchdog.ttl),
                self.watchdog.remaining(),
            );
            sleep(pace).await;
        }
    }

    fn lost(&self, reason: LeaseLossReason, source: Option<etcd_client::Error>) -> LeaseLost {
        LeaseLost {
            lease_id: self.lease_id,
            reason,
            source,
        }
    }
}

/// This replica won the campaign and holds the lease. Nothing here revokes or resigns on drop: a
/// killed leader must be indistinguishable from a crashed one, so failover is always measured
/// against the TTL. A `Drop` impl is a compile error at the partial moves; the failover test's
/// lower bound catches a revoke issued from anywhere else.
pub struct Elected {
    pub fence: LeaderFence,
    pub lease_id: i64,
    /// What etcd actually granted, which can exceed what was asked for.
    pub granted_ttl: Duration,
    pub keepalive: LeaseKeepAlive,
    pub leadership: LeadershipHandle,
}

/// How fast a retriable etcd failure is retried: from `min`, doubling, up to `max`.
#[derive(Clone, Copy, Debug)]
pub struct RetryDelays {
    pub min: Duration,
    pub max: Duration,
}

impl RetryDelays {
    pub fn from_config(config: &EtcdConfig) -> Self {
        Self {
            min: config.retry_min_delay,
            max: config.retry_max_delay,
        }
    }
}

/// Releases the leadership on request. Cloned out of [`Elected`] before its other fields move,
/// so a shutdown can hand over while the keepalive task still renews.
#[derive(Clone)]
pub struct LeadershipHandle {
    lease_client: LeaseClient,
    lease_id: i64,
    request_timeout: Duration,
    /// Shared with every clone, so the keepalive task can tell a revoke it asked for from one it
    /// did not.
    stepped_down: Arc<AtomicBool>,
    /// For `publish_ready`: the election channel, the key this replica won, its value, and how
    /// long and how fast a retriable failure is retried.
    client: Client,
    leader_key: LeaderKey,
    value: ElectionValue,
    ready_budget: Duration,
    retry: RetryDelays,
}

impl LeadershipHandle {
    /// Marks this leader's election value ready, which is when the standbys start sending clients
    /// here. Called once its gRPC service serves.
    ///
    /// Retried, unlike the other writes in distributed mode: it only sets this replica's own value
    /// again, so a retriable failure - a member that refuses connections among several endpoints,
    /// say - is retried within one lease TTL, past which the lease is lost anyway. etcd refuses it
    /// for good once the key is no longer this replica's, and that fails the startup, which steps
    /// down.
    ///
    /// `lease` is the belief about the lease that the keepalive maintains. It is checked before
    /// every attempt, not once before the first, because the keepalive reports a lost lease only
    /// when it next runs and a retry that sleeps in between can outlast the lease: a lapsed lease
    /// ends the publication with an error that is not retried, so that a replica that no longer
    /// leads never announces itself as the leader.
    pub async fn publish_ready(
        &self,
        lease: Option<&LeaseBelief>,
    ) -> Result<(), ShardManagerError> {
        let value = ElectionValue {
            ready: true,
            ..self.value.clone()
        }
        .encode();
        let deadline = Instant::now()
            .checked_add(self.ready_budget)
            .unwrap_or_else(Instant::now);
        retry_retriable_until(
            "marking the shard manager leader ready",
            || {
                let mut election_client = self.client.election_client();
                let options = ProclaimOptions::new().with_leader(self.leader_key.clone());
                let value = value.clone();
                let lapsed = lease.is_some_and(|belief| !belief.held());
                async move {
                    if lapsed {
                        return Err(ShardManagerError::Internal(format!(
                            "the leadership lease {} lapsed before this replica could publish \
                             itself as the leader",
                            self.lease_id
                        )));
                    }
                    bounded(
                        self.request_timeout,
                        election_client.proclaim(value, Some(options)),
                    )
                    .await
                    .map(|_| ())
                }
            },
            deadline,
            self.retry.min,
            self.retry.max,
        )
        .await
    }

    /// Revokes the lease, which deletes the election key: without it a stopped replica costs the
    /// deployment a full TTL with no routing table.
    pub async fn step_down(&self) -> Result<(), ShardManagerError> {
        // Set before the revoke and the gauge, so the keepalive cannot observe the loss ahead of
        // the intent. Every caller is on its way out: a failed revoke is a lease left to lapse.
        self.stepped_down.store(true, Ordering::SeqCst);
        metrics::record_standing_by();
        // A lease etcd no longer knows is already released; anything else is reported and the
        // lease is left to lapse on its own.
        let lease_id = self.lease_id;
        let mut lease_client = self.lease_client.clone();
        match bounded(self.request_timeout, lease_client.revoke(lease_id)).await {
            Ok(_) => {}
            Err(err) if is_lease_not_found(&err) => {
                info!(lease_id, "The leadership lease was already gone");
                return Ok(());
            }
            Err(err) => return Err(err),
        }
        info!(lease_id, "Released the shard manager leadership");
        Ok(())
    }

    /// Whether [`Self::step_down`] has been called on this handle or any of its clones.
    pub fn has_stepped_down(&self) -> bool {
        self.stepped_down.load(Ordering::SeqCst)
    }
}

/// Replaces the result of a confirming read, by attempt number. **Tests only**; see
/// [`LeaderElection::with_confirm_hook`].
type ConfirmHook = Arc<dyn Fn(u32) -> Option<ShardManagerError> + Send + Sync>;

/// Why a waiting campaign stopped without being a failure of the service. The campaign decides
/// what each one means: see [`LeaderElection::campaign_until_elected`].
#[derive(Debug, thiserror::Error)]
enum CampaignInterrupt {
    /// A campaigner of a newer version queues behind this replica, so this one gives way and
    /// queues again at the end.
    #[error("moved to the end of the election queue behind version {newer} (own: {own})")]
    YieldToNewer { own: String, newer: String },
    /// Another live replica advertises this replica's gRPC address; only a change of
    /// configuration helps.
    #[error(
        "another shard manager replica ({other}) advertises the same gRPC address {address}. Every \
         replica needs its own grpc.advertised_host, such as its pod IP; never a Service name"
    )]
    DuplicateAddress { address: String, other: String },
}

/// How one campaign attempt ended without winning.
enum CampaignExit {
    Failed(ShardManagerError),
    Interrupted(CampaignInterrupt),
}

impl From<ShardManagerError> for CampaignExit {
    fn from(err: ShardManagerError) -> Self {
        Self::Failed(err)
    }
}

impl From<CampaignInterrupt> for CampaignExit {
    fn from(interrupt: CampaignInterrupt) -> Self {
        Self::Interrupted(interrupt)
    }
}

pub struct LeaderElection {
    client: Client,
    election_name: String,
    lease_ttl: Duration,
    request_timeout: Duration,
    retry: RetryDelays,
    standby_log_interval: Duration,
    value: ElectionValue,
    shutdown: CancellationToken,
    confirm_hook: Option<ConfirmHook>,
    /// Counts every confirming read this election has made, across campaigns, so that the hook can
    /// name one attempt out of the whole run rather than one per campaign.
    confirm_attempts: AtomicU32,
    /// The lease of the attempt in flight, or 0. Set after the grant and cleared when the attempt
    /// finishes, so a caller that drops the campaign can still revoke what it left behind.
    pending_lease: AtomicI64,
    /// The watched view of the election, when the replica has one: the campaign checks it for a
    /// replica that advertises the same address.
    watch: Option<ElectionWatch>,
}

impl LeaderElection {
    pub async fn connect(
        config: &EtcdConfig,
        election_name: impl Into<String>,
        grpc_address: LeaderAddress,
    ) -> Result<Self, ShardManagerError> {
        Ok(Self {
            client: connect_for_election(config).await?,
            election_name: election_name.into(),
            lease_ttl: config.leader_lease_ttl,
            request_timeout: config.request_timeout,
            retry: RetryDelays::from_config(config),
            standby_log_interval: STANDBY_LOG_INTERVAL,
            value: ElectionValue {
                identity: identity(),
                grpc_address,
                ready: false,
                version: golem_common::golem_version().to_string(),
            },
            shutdown: CancellationToken::new(),
            confirm_hook: None,
            confirm_attempts: AtomicU32::new(0),
            pending_lease: AtomicI64::new(0),
            watch: None,
        })
    }

    /// Ends the campaign when `shutdown` is cancelled. A standby killed while queued leaves its key
    /// in etcd's FIFO for a full TTL, and the next election waits behind it even after a clean
    /// handover.
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Replaces the build version that this replica announces in its election value. **Tests
    /// only**: the replicas of one test process share one build, so only this lets a test queue
    /// an old and a new version together.
    #[doc(hidden)]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.value.version = version.into();
        self
    }

    /// Checks the campaign against the watched view while this replica waits: see
    /// [`Self::watch_campaign`].
    pub fn with_watch(mut self, watch: ElectionWatch) -> Self {
        self.watch = Some(watch);
        self
    }

    /// Shortens the standby announcement cadence. **Tests only** - the production cadence is
    /// [`STANDBY_LOG_INTERVAL`].
    #[doc(hidden)]
    pub fn with_standby_log_interval(mut self, interval: Duration) -> Self {
        self.standby_log_interval = interval;
        self
    }

    /// Fails a confirming read that would otherwise succeed. **Tests only**: called with the read's
    /// number, counting from one across the election; a `Some` becomes that read's result. The
    /// fault it simulates lasts milliseconds and no test can provoke it from outside.
    #[doc(hidden)]
    pub fn with_confirm_hook(
        mut self,
        hook: impl Fn(u32) -> Option<ShardManagerError> + Send + Sync + 'static,
    ) -> Self {
        self.confirm_hook = Some(Arc::new(hook));
        self
    }

    /// Blocks until elected, renewing throughout. Retries retriable failures indefinitely: waiting
    /// is a standby's entire job.
    pub async fn campaign_until_elected(&self) -> Result<Elected, ShardManagerError> {
        info!(
            election = self.election_name,
            identity = self.value.identity,
            lease_ttl = ?self.lease_ttl,
            "Campaigning for shard manager leadership"
        );

        self.campaign_loop().await
    }

    async fn campaign_loop(&self) -> Result<Elected, ShardManagerError> {
        let mut backoff = self.retry.min;

        loop {
            if self.shutdown.is_cancelled() {
                return Err(ShardManagerError::ShutdownRequested);
            }

            match self.campaign_once().await {
                Ok(elected) => return Ok(elected),
                // Not a failure: the lease was revoked with the key, and the next campaign queues
                // at the end, behind the newer replica.
                Err(CampaignExit::Interrupted(CampaignInterrupt::YieldToNewer { own, newer })) => {
                    info!(
                        own,
                        newer, "Moved behind a newer version in the election queue"
                    );
                }
                // The other replica keeps renewing its key, so a new campaign would meet it
                // again: this ends the startup.
                Err(CampaignExit::Interrupted(duplicate)) => {
                    return Err(ShardManagerError::InvalidConfig(duplicate.to_string()));
                }
                Err(CampaignExit::Failed(err)) if err.is_retriable() => {
                    metrics::record_campaign_attempt_failure();
                    warn!(error = %err, retry_in = ?backoff, "Campaign attempt failed; retrying");
                    tokio::select! {
                        _ = sleep_until(Instant::now() + backoff) => {}
                        _ = self.shutdown.cancelled() => {
                            return Err(ShardManagerError::ShutdownRequested);
                        }
                    }
                    backoff = std::cmp::min(backoff.saturating_mul(2), self.retry.max);
                }
                Err(CampaignExit::Failed(err)) => return Err(err),
            }
        }
    }

    async fn campaign_once(&self) -> Result<Elected, CampaignExit> {
        let mut lease_client = self.client.lease_client();

        let ttl_secs = self.lease_ttl.as_secs() as i64;
        let grant = self.bounded(lease_client.grant(ttl_secs, None)).await?;
        let lease_id = grant.id();
        self.pending_lease.store(lease_id, Ordering::SeqCst);
        let granted_ttl = Duration::from_secs(grant.ttl().max(0) as u64);
        if granted_ttl != self.lease_ttl {
            warn!(
                requested = ?self.lease_ttl,
                granted = ?granted_ttl,
                "etcd adjusted the leadership lease TTL to its own minimum"
            );
        }

        match self
            .campaign_with_lease(&mut lease_client, lease_id, granted_ttl)
            .await
        {
            Ok(elected) => {
                // The lease now belongs to the leadership, which releases it.
                self.pending_lease.store(0, Ordering::SeqCst);
                Ok(elected)
            }
            Err(err) => {
                // Every failure past the grant comes back here. A lease left held parks the
                // server's queue for a full TTL - and once the campaign is won, it is holding the
                // election itself. The slot is cleared only after the revoke, so an attempt dropped
                // during it still leaves the lease findable.
                if let Err(revoke_failed) = self.bounded(lease_client.revoke(lease_id)).await {
                    warn!(
                        error = %revoke_failed,
                        lease_id,
                        "Cannot revoke the lease of a failed campaign attempt; the election key \
                         attached to it will block the queue until the lease expires"
                    );
                }
                self.pending_lease.store(0, Ordering::SeqCst);
                Err(err)
            }
        }
    }

    /// Everything a campaign does with a lease that has already been granted; see the revoke in
    /// [`Self::campaign_once`].
    async fn campaign_with_lease(
        &self,
        lease_client: &mut LeaseClient,
        lease_id: i64,
        granted_ttl: Duration,
    ) -> Result<Elected, CampaignExit> {
        let handshake_sent = SentAt::now();
        let (keeper, stream) = self.bounded(lease_client.keep_alive(lease_id)).await?;
        let mut keepalive = LeaseKeepAlive {
            lease_id,
            keeper,
            stream,
            watchdog: RenewalWatchdog::armed_at(handshake_sent, granted_ttl),
            lease_client: lease_client.clone(),
            request_timeout: self.request_timeout,
        };

        let mut election_client = self.client.election_client();
        let campaign =
            election_client.campaign(self.election_name.clone(), self.value.encode(), lease_id);

        // The keepalive runs *during* the campaign: etcd's election server orphans the session it
        // builds from this lease, so nothing renews it server-side, and a standby that waited would
        // watch its own lease expire mid-wait. The announcement is an arm rather than awaited in an
        // arm body so it can never delay a renewal or the campaign.
        let response = tokio::select! {
            biased;
            lost = keepalive.drive() => {
                return Err(ShardManagerError::LeaseLostWhileCampaigning(lost).into());
            }
            won = campaign => won.map_err(ShardManagerError::from)?,
            interrupted = self.watch_campaign(lease_id, granted_ttl) => return Err(interrupted.into()),
            _ = self.shutdown.cancelled() => return Err(ShardManagerError::ShutdownRequested.into()),
            never = self.announce_standby() => match never {},
        };

        let leader = response.leader().ok_or_else(|| {
            ShardManagerError::Internal("etcd campaign response carried no leader key".to_string())
        })?;
        let fence = LeaderFence::from_leader_key(leader)?;

        // The confirming read is raced against the keepalive: it may take several attempts, the
        // lease has to go on being renewed underneath them, and a real loss must still end the
        // campaign. Its budget leaves one renewal interval of lease to revoke on giving up.
        let confirm_budget =
            granted_ttl.saturating_sub(LeaseKeepAlive::renewal_interval(granted_ttl));
        tokio::select! {
            biased;
            lost = keepalive.drive() => {
                return Err(ShardManagerError::LeaseLostWhileCampaigning(lost).into());
            }
            // Leaves through `campaign_once`'s revoke like every other error, so a shutdown here
            // releases the lease the win is holding rather than waiting the confirm out.
            _ = self.shutdown.cancelled() => return Err(ShardManagerError::ShutdownRequested.into()),
            confirmed = self.confirm_fence_is_live(&fence, lease_id, confirm_budget) => confirmed?,
        }

        info!(
            leader_key = fence.key_str(),
            create_revision = fence.create_revision(),
            lease_id,
            granted_ttl = ?granted_ttl,
            identity = self.value.identity,
            "Elected as the shard manager leader"
        );

        Ok(Elected {
            fence,
            lease_id,
            granted_ttl,
            keepalive,
            leadership: LeadershipHandle {
                lease_client: lease_client.clone(),
                lease_id,
                request_timeout: self.request_timeout,
                stepped_down: Arc::new(AtomicBool::new(false)),
                client: self.client.clone(),
                leader_key: leader.clone(),
                value: self.value.clone(),
                ready_budget: granted_ttl,
                retry: self.retry,
            },
        })
    }

    /// Rejects a win that is already over. etcd's campaign returns once the key ahead is deleted
    /// and never re-checks the campaigner's own key, so a replica whose lease died at that moment
    /// is handed a fence over a key that no longer exists. Only a definitive answer rejects the
    /// win; retriable read failures are retried within `budget`, and a spent budget reports the
    /// original failure.
    async fn confirm_fence_is_live(
        &self,
        fence: &LeaderFence,
        lease_id: i64,
        budget: Duration,
    ) -> Result<(), ShardManagerError> {
        let deadline = Instant::now().checked_add(budget).ok_or_else(|| {
            ShardManagerError::Internal(
                "Leader fence confirmation budget exceeds the clock range".to_string(),
            )
        })?;
        let response = retry_retriable_until(
            "confirming the won leader key",
            || {
                let attempt = self.confirm_attempts.fetch_add(1, Ordering::SeqCst) + 1;
                let injected = self.confirm_hook.as_ref().and_then(|hook| hook(attempt));
                let mut kv_client = self.client.kv_client();
                let key = fence.key().to_vec();
                async move {
                    match injected {
                        Some(err) => Err(err),
                        None => self.bounded(kv_client.get(key, None)).await,
                    }
                }
            },
            deadline,
            self.retry.min,
            self.retry.max,
        )
        .await?;

        let held = response
            .kvs()
            .first()
            .is_some_and(|kv| kv.create_revision() == fence.create_revision());
        if !held {
            return Err(ShardManagerError::LeaseLostWhileCampaigning(LeaseLost {
                lease_id,
                reason: LeaseLossReason::LeaderKeyGone,
                source: None,
            }));
        }

        Ok(())
    }

    /// Logs who holds the leadership, on a fixed cadence, for as long as it is polled.
    async fn announce_standby(&self) -> Infallible {
        let mut announce = interval(self.standby_log_interval);
        announce.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick is immediate, and a campaign that is about to be won is not a standby.
        announce.tick().await;

        loop {
            announce.tick().await;
            match self.current_leader().await {
                Ok(Some(leader)) => info!(
                    leader = leader.identity,
                    leader_address = leader.grpc_address.to_string(),
                    leader_ready = leader.ready,
                    "Still standing by; leadership is held elsewhere"
                ),
                Ok(None) => info!("Still standing by; no leader is currently recorded"),
                Err(err) => warn!(error = %err, "Still standing by; cannot read the leader"),
            }
        }
    }

    /// Ends a waiting campaign when the watched view shows that it must not go on: a campaigner
    /// of a newer version queues behind this replica (see [`newer_version_behind`]), or another
    /// live campaigner advertises this replica's address. Never returns without a watch.
    ///
    /// A key of an earlier process of the same pod also carries this address, but nothing renews
    /// its lease, so the lease only counts down until the key disappears. A live duplicate
    /// renews its lease, which moves its end later. The campaign therefore reads the other key's
    /// lease at every recheck and refuses the duplicate only after it has seen the end move
    /// later twice (see [`RenewalEvidence`]); how long the lease would otherwise last decides
    /// nothing, because a change of etcd's raft leader extends every lease once and an abandoned
    /// key can then outlive any fixed patience. A live replica renews at a third of its lease,
    /// so it is seen renewing within about one lease.
    ///
    /// A lease that is already gone is never refused, because its key is about to disappear with
    /// it, and neither is a key whose lease could not be read: the read is tried again on the next
    /// recheck. The recheck comes at most every quarter of this replica's TTL, and at most every
    /// quarter of the other lease's granted TTL once that is known, which bounds how soon a live
    /// duplicate is found.
    async fn watch_campaign(&self, own_lease: i64, granted_ttl: Duration) -> CampaignInterrupt {
        let Some(mut watch) = self.watch.clone() else {
            return std::future::pending().await;
        };
        let own = &self.value.grpc_address;
        let mut duplicate: Option<SeenDuplicate> = None;
        loop {
            let mut recheck = granted_ttl / 4;
            if let Some(view) = watch.view() {
                if let Some(newer) = newer_version_behind(&view, own_lease, &self.value.version) {
                    return CampaignInterrupt::YieldToNewer {
                        own: self.value.version.clone(),
                        newer,
                    };
                }
                match duplicate_of(&view, own_lease, own) {
                    Some(other) => {
                        let seen = match &mut duplicate {
                            Some(seen) if seen.key == other.key => seen,
                            _ => {
                                warn!(
                                    address = %own,
                                    other = other.value.as_ref().map(|value| value.identity.as_str()),
                                    "Another shard manager replica advertises this replica's gRPC address"
                                );
                                duplicate.insert(SeenDuplicate {
                                    key: other.key.clone(),
                                    evidence: RenewalEvidence::default(),
                                    other_granted_ttl: None,
                                })
                            }
                        };
                        let sent = Instant::now();
                        let read = self.read_lease(other.lease).await;
                        let received = Instant::now();
                        match read {
                            LeaseRead::Alive {
                                remaining,
                                granted,
                                term,
                            } => {
                                seen.other_granted_ttl = Some(granted);
                                if seen.evidence.observe(sent, received, remaining, term) {
                                    return CampaignInterrupt::DuplicateAddress {
                                        address: own.to_string(),
                                        other: other
                                            .value
                                            .as_ref()
                                            .map(|value| value.identity.clone())
                                            .unwrap_or_default(),
                                    };
                                }
                            }
                            LeaseRead::Gone => seen.evidence = RenewalEvidence::default(),
                            LeaseRead::Unknown => {}
                        }
                        if let Some(granted) = seen.other_granted_ttl {
                            recheck = recheck.min(granted / 4);
                        }
                    }
                    None => duplicate = None,
                }
            }
            // A zero wait would spin.
            let recheck = recheck.max(Duration::from_millis(50));
            tokio::select! {
                _ = watch.changed() => {}
                _ = sleep(recheck) => {}
            }
        }
    }

    /// What etcd says about `lease`: the time left on it and the lifetime it was granted, or that
    /// it no longer exists, or nothing when the read failed.
    async fn read_lease(&self, lease: i64) -> LeaseRead {
        let mut lease_client = self.client.lease_client();
        match self.bounded(lease_client.time_to_live(lease, None)).await {
            Ok(response) if response.ttl() < 0 => {
                debug!(
                    lease,
                    "The lease of a duplicate key is gone; its key is about to disappear"
                );
                LeaseRead::Gone
            }
            Ok(response) => {
                let term = response.header().map_or(0, |header| header.raft_term());
                let read = LeaseRead::of_answer(response.ttl(), response.granted_ttl(), term);
                if matches!(read, LeaseRead::Unknown) {
                    warn!(
                        lease,
                        remaining = response.ttl(),
                        granted = response.granted_ttl(),
                        "A duplicate key's lease has more time left than it was granted; ignoring \
                         the answer"
                    );
                }
                read
            }
            Err(err) => {
                warn!(
                    lease,
                    error = %err,
                    "Cannot read a duplicate key's lease; trying again"
                );
                LeaseRead::Unknown
            }
        }
    }

    /// The view of this election and the task that keeps it current. The task must be spawned;
    /// it runs until the replica stops.
    pub fn watch(&self) -> (ElectionWatch, ElectionWatcher) {
        ElectionWatcher::new(
            self.client.clone(),
            &self.election_name,
            self.request_timeout,
            self.lease_ttl,
            self.retry,
        )
    }

    /// Bounds one request on the election channel, which has no channel-level timeout: only the
    /// campaign may block indefinitely.
    async fn bounded<T, F>(&self, request: F) -> Result<T, ShardManagerError>
    where
        F: Future<Output = Result<T, etcd_client::Error>>,
    {
        bounded(self.request_timeout, request).await
    }

    /// Revokes the lease of an attempt that was dropped rather than returned.
    pub async fn revoke_pending_lease(&self) {
        let lease_id = self.pending_lease.swap(0, Ordering::SeqCst);
        if lease_id == 0 {
            return;
        }

        let mut lease_client = self.client.lease_client();
        match self.bounded(lease_client.revoke(lease_id)).await {
            Ok(_) => info!(lease_id, "Revoked the lease of an abandoned campaign"),
            Err(err) if is_lease_not_found(&err) => {}
            Err(err) => warn!(
                error = %err,
                lease_id,
                "Cannot revoke the lease of an abandoned campaign; any election key on it will \
                 block the queue until the lease expires"
            ),
        }
    }

    /// The value recorded by whichever replica currently holds the leadership.
    pub async fn current_leader(&self) -> Result<Option<ElectionValue>, ShardManagerError> {
        LeaderLocator::new(
            self.client.clone(),
            self.election_name.clone(),
            self.request_timeout,
        )
        .leader()
        .await
    }
}

/// etcd's answer to revoking a lease it no longer holds; the leadership is released either way.
fn is_lease_not_found(err: &ShardManagerError) -> bool {
    matches!(
        err,
        ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(status))
            if status.message().contains("requested lease not found")
    )
}

/// `etcd-client`'s own answer when a keepalive handshake is met with a zero TTL.
fn is_lease_gone(err: &etcd_client::Error) -> bool {
    matches!(err, etcd_client::Error::LeaseKeepAliveError(message) if message.contains("lease not found"))
}

/// Whether etcd refused a leader lookup because the election has no leader.
fn is_no_leader(err: &ShardManagerError) -> bool {
    matches!(
        err,
        ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(status))
            if status.message().contains("no leader")
    )
}

pub(crate) async fn bounded<T, F>(
    request_timeout: Duration,
    request: F,
) -> Result<T, ShardManagerError>
where
    F: Future<Output = Result<T, etcd_client::Error>>,
{
    timeout(request_timeout, request)
        .await
        .map_err(|_| ShardManagerError::Timeout)?
        .map_err(ShardManagerError::from)
}

/// What a replica records under its election key. JSON, so that `etcdctl get` shows it.
///
/// Every replica writes its value when it joins the campaign, with `ready` false. The leader
/// rewrites its own value with `ready` true once its startup has completed
/// ([`LeadershipHandle::publish_ready`]), and a standby names only a ready leader, so a client is
/// never sent to a leader that cannot serve yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElectionValue {
    /// Diagnostic only; see [`identity`].
    pub identity: String,
    /// Where this replica serves gRPC to other processes. A value without it is not an
    /// `ElectionValue`: see [`Self::decode`].
    pub grpc_address: LeaderAddress,
    pub ready: bool,
    /// The build version of this replica. A standby moves behind a campaigner of a newer version,
    /// so that a rolling update hands the leadership to a new replica. A value written by a build
    /// that announced none reads as an empty text, which is never newer.
    #[serde(default)]
    pub version: String,
}

impl ElectionValue {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("an election value always serializes")
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

/// Reads the current leader's election value: how a standby learns where to send clients.
#[derive(Clone)]
pub struct LeaderLocator {
    client: Client,
    election_name: String,
    request_timeout: Duration,
}

impl LeaderLocator {
    pub fn new(
        client: Client,
        election_name: impl Into<String>,
        request_timeout: Duration,
    ) -> Self {
        Self {
            client,
            election_name: election_name.into(),
            request_timeout,
        }
    }

    /// `None` while no replica leads.
    pub async fn leader(&self) -> Result<Option<ElectionValue>, ShardManagerError> {
        let mut election_client = self.client.election_client();
        let response = match bounded(
            self.request_timeout,
            election_client.leader(self.election_name.clone()),
        )
        .await
        {
            Ok(response) => response,
            // etcd reports a leaderless election as an error with the same `Unknown` code as a
            // transport fault; the message is the only thing that tells the two apart.
            Err(err) if is_no_leader(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        response
            .kv()
            .map(|kv| {
                ElectionValue::decode(kv.value()).ok_or_else(|| {
                    ShardManagerError::Internal(format!(
                        "the shard manager leader's election value is not readable: {}",
                        String::from_utf8_lossy(kv.value())
                    ))
                })
            })
            .transpose()
    }
}

/// The version of a campaigner queued behind this replica that is newer than this replica's own,
/// if any (the newest of them). The head of the queue never moves: it is about to lead. A version
/// is newer only when both parse (see [`version_order`]), and then by semantic-version precedence
/// (build metadata is ignored).
///
/// Golem's own tags are `X.Y.Z-rcN`, and `rc10` follows `rc9`: the number of a candidate is
/// compared as a number, not as text. A development build announces the `git describe` text of
/// its last tag, such as `1.6.0-rc2-156-g2917a80d2`, which compares as a pre-release of the
/// release that tag belongs to: newer than `1.6.0-rc2` (and than a build with fewer commits after
/// it), older than `1.6.0-rc3` and than `1.6.0`. A build made after a release tag
/// (`1.6.0-12-g2917a80d2`) is newer than every candidate of that release and older than the
/// release itself. A replica that announces nothing, or a text that is no semantic version, is
/// never newer and never moves.
pub(crate) fn newer_version_behind(
    view: &CampaignView,
    own_lease: i64,
    own_version: &str,
) -> Option<String> {
    let own_version = version_order(own_version)?;
    let position = view
        .campaigners
        .iter()
        .position(|campaigner| campaigner.lease == own_lease)?;
    if position == 0 {
        return None;
    }
    view.campaigners[position + 1..]
        .iter()
        .filter_map(|campaigner| {
            let version = &campaigner.value.as_ref()?.version;
            let parsed = version_order(version)?;
            (parsed.cmp_precedence(&own_version) == std::cmp::Ordering::Greater)
                .then_some((parsed, version))
        })
        .max_by(|(left, _), (right, _)| left.cmp_precedence(right))
        .map(|(_, version)| version.clone())
}

/// The version in the form that orders as Golem's releases do, or `None` for a text that is no
/// semantic version.
///
/// Semantic-version precedence compares an identifier with letters as text, so `rc10` would come
/// before `rc9`. Two shapes that Golem produces are therefore rewritten into numeric identifiers
/// first: a candidate `rcN` becomes `rc.N` (so `rc2.2` becomes `rc.2.2`), and the commit count of
/// a `git describe` suffix `-K-gHASH` becomes the numeric identifier `K`, with the hash dropped.
/// A suffix that follows a release tag directly has no candidate before it and gets the identifier
/// `release` in front, which sorts after `rc`: such a build is newer than every candidate and,
/// as a pre-release, still older than the release. Anything else is compared as it is written.
fn version_order(text: &str) -> Option<semver::Version> {
    let mut version = semver::Version::parse(text).ok()?;
    let pre = version.pre.as_str().to_string();
    let (base, commits) = match describe_suffix(&pre) {
        Some((base, commits)) => (base, Some(commits)),
        None => (pre.as_str(), None),
    };
    let mut identifiers: Vec<String> = Vec::new();
    if commits.is_some() && base.is_empty() {
        identifiers.push("release".to_string());
    }
    for identifier in base.split('.').filter(|identifier| !identifier.is_empty()) {
        match identifier.strip_prefix("rc") {
            Some(number)
                if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                identifiers.push("rc".to_string());
                identifiers.push(number.to_string());
            }
            _ => identifiers.push(identifier.to_string()),
        }
    }
    if let Some(commits) = commits {
        identifiers.push(commits.to_string());
    }
    version.pre = semver::Prerelease::new(&identifiers.join(".")).ok()?;
    Some(version)
}

/// Splits `BASE-K-gHASH` (or `K-gHASH`, with an empty base) into the base and the count `K`.
fn describe_suffix(pre: &str) -> Option<(&str, u64)> {
    let (before_hash, hash) = pre.rsplit_once("-g")?;
    if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let (base, commits) = before_hash.rsplit_once('-').unwrap_or(("", before_hash));
    if commits.is_empty() || !commits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((base, commits.parse().ok()?))
}

/// The key of another campaigner that advertises this replica's address, what has been seen of
/// its lease renewing, and the lifetime that lease was granted once that has been read.
struct SeenDuplicate {
    key: Vec<u8>,
    evidence: RenewalEvidence,
    other_granted_ttl: Option<Duration>,
}

/// One read of another campaigner's lease.
enum LeaseRead {
    Alive {
        remaining: i64,
        granted: Duration,
        /// The raft term of etcd's answer, 0 when the answer carried no header.
        term: u64,
    },
    Gone,
    Unknown,
}

impl LeaseRead {
    /// How long a lease can have left beyond the time it was granted: a renewal that races the
    /// read, and the second etcd's truncation hides.
    const SLACK: Duration = Duration::from_secs(2);

    /// An answer with more time left than the lease was granted, such as etcd's "no expiry"
    /// value, says nothing about a renewal and would hold the evidence beyond every later read, so
    /// it is treated like a failed read.
    fn of_answer(remaining: i64, granted: i64, term: u64) -> Self {
        let granted = Duration::from_secs(granted.max(0) as u64);
        let remaining = Duration::from_secs(remaining.max(0) as u64);
        if remaining > granted + Self::SLACK {
            return Self::Unknown;
        }
        Self::Alive {
            remaining: remaining.as_secs() as i64,
            granted,
            term,
        }
    }
}

/// Counts how often a lease was seen to end later than it could have ended without a renewal.
///
/// etcd answers a read of a lease in whole seconds, truncated, so the time left on a lease that is
/// renewed often can read the same on every read; only its end can be compared. A read sent at
/// `sent` and answered at `received` with `ttl` says that the lease ends no earlier than
/// `sent + ttl` and no later than `received + ttl + 1 s`. The end of an abandoned lease is fixed,
/// so the earliest end that a later read allows can never pass the latest end that an earlier
/// read allowed. When it does, someone extended the lease: its owner renewed it, or etcd's raft
/// leader changed, which extends every lease once.
///
/// Every answer carries the raft term of the etcd member that gave it, and each change of leader
/// starts a new term, so extensions are only compared between two reads of one term. A read of a
/// new term starts the count again from that read: however many leader changes an abandoned lease
/// lives through, each can only give it one extension, and none of them is counted. A live owner
/// renews many times in one term, so two extensions within one term still prove it. One
/// extension within a term proves nothing and two prove that someone renews.
#[derive(Debug, Default)]
pub(crate) struct RenewalEvidence {
    /// The latest the lease can end with no further extension, as of the read after the last
    /// extension that was counted.
    unextended_end: Option<Instant>,
    /// The raft term of the read that set `unextended_end`.
    term: u64,
    extensions: u32,
}

impl RenewalEvidence {
    const EXTENSIONS_THAT_PROVE_A_RENEWAL: u32 = 2;

    /// Records a read that was sent at `sent` and answered at `received` with the whole seconds
    /// `ttl`, in raft term `term`. A term of zero is an answer without a header and, like a failed
    /// read, changes nothing. Returns whether the evidence now proves that the lease is renewed.
    pub(crate) fn observe(
        &mut self,
        sent: Instant,
        received: Instant,
        ttl: i64,
        term: u64,
    ) -> bool {
        if term == 0 {
            return self.proven();
        }
        let ttl = Duration::from_secs(ttl.max(0) as u64);
        // An answer too large for an instant is no answer that a lease can give.
        let (Some(earliest_end), Some(latest_end)) = (
            sent.checked_add(ttl),
            received
                .checked_add(ttl)
                .and_then(|end| end.checked_add(Duration::from_secs(1))),
        ) else {
            return self.proven();
        };
        match self.unextended_end {
            None => {
                self.unextended_end = Some(latest_end);
                self.term = term;
            }
            // Extensions of an earlier term, and a leader change that happened since the anchor,
            // are not a renewal: start again from this read.
            Some(_) if term != self.term => {
                self.unextended_end = Some(latest_end);
                self.term = term;
                self.extensions = 0;
            }
            // An answer of zero is a lease that ran out and is not yet revoked: it proves nothing.
            Some(end) if !ttl.is_zero() && earliest_end > end => {
                self.extensions += 1;
                self.unextended_end = Some(latest_end);
            }
            Some(_) => {}
        }
        self.proven()
    }

    fn proven(&self) -> bool {
        self.extensions >= Self::EXTENSIONS_THAT_PROVE_A_RENEWAL
    }
}

/// Another campaigner that advertises `own`. The own key is told apart by its lease.
pub(crate) fn duplicate_of<'a>(
    view: &'a CampaignView,
    own_lease: i64,
    own: &LeaderAddress,
) -> Option<&'a Campaigner> {
    view.campaigners.iter().find(|campaigner| {
        campaigner.lease != own_lease
            && campaigner
                .value
                .as_ref()
                .is_some_and(|value| &value.grpc_address == own)
    })
}

/// This replica's identity in its election value, diagnostic only. The pid and start time answer
/// the failover-time question "is that key the old pod, or my own from before the restart?".
fn identity() -> String {
    let host = std::env::var("POD_NAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_string());

    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();

    format!("{host}/pid-{}/{started_at}", std::process::id())
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    const TTL: Duration = Duration::from_secs(10);

    fn campaigner(lease: i64, revision: i64, port: u16) -> Campaigner {
        Campaigner {
            key: format!("e/{lease:x}").into_bytes(),
            lease,
            create_revision: revision,
            value: Some(ElectionValue {
                identity: format!("sm-{lease}"),
                grpc_address: LeaderAddress {
                    host: "10.0.0.1".to_string(),
                    port,
                },
                ready: false,
                version: String::new(),
            }),
        }
    }

    fn own() -> LeaderAddress {
        LeaderAddress {
            host: "10.0.0.1".to_string(),
            port: 9001,
        }
    }

    fn with_version(mut campaigner: Campaigner, version: &str) -> Campaigner {
        if let Some(value) = campaigner.value.as_mut() {
            value.version = version.to_string();
        }
        campaigner
    }

    /// A queue of three: the head, the replica under test (lease 2) and a last one of `last`.
    fn queue_ending_in(last: &str) -> CampaignView {
        CampaignView {
            campaigners: vec![
                with_version(campaigner(1, 10, 9001), "1.5.0"),
                with_version(campaigner(2, 20, 9002), "1.5.0"),
                with_version(campaigner(3, 30, 9003), last),
            ],
        }
    }

    #[test]
    fn a_standby_moves_for_a_newer_version_behind_it() {
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0"), 2, "1.5.0"),
            Some("1.6.0".to_string())
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.5.1-rc.1"), 2, "1.5.0"),
            Some("1.5.1-rc.1".to_string())
        );
    }

    /// Whether the version `older` orders strictly before `newer` in the replica queue.
    fn orders_before(older: &str, newer: &str) -> bool {
        newer_version_behind(&queue_ending_in(newer), 2, older) == Some(newer.to_string())
            && newer_version_behind(&queue_ending_in(older), 2, newer).is_none()
    }

    #[test]
    fn a_candidate_number_is_compared_as_a_number() {
        assert!(orders_before("1.6.0-rc9", "1.6.0-rc10"));
        assert!(orders_before("1.6.0-rc10", "1.6.0-rc11"));
        assert!(orders_before("1.6.0-rc2.2", "1.6.0-rc10"));
        assert!(orders_before("1.6.0-rc2", "1.6.0-rc2.2"));
    }

    #[test]
    fn builds_after_a_candidate_order_by_their_commit_count() {
        let order = [
            "1.6.0-rc2",
            "1.6.0-rc2-1-g0123abcde",
            "1.6.0-rc2-99-gfedcba987",
            "1.6.0-rc2-156-g2917a80d2",
            "1.6.0-rc3",
            "1.6.0",
        ];
        for (index, older) in order.iter().enumerate() {
            for newer in &order[index + 1..] {
                assert!(orders_before(older, newer), "{older} before {newer}");
            }
        }
    }

    #[test]
    fn a_build_after_a_release_tag_is_between_its_last_candidate_and_the_release() {
        assert!(orders_before("1.6.0-rc11", "1.6.0-12-g2917a80d2"));
        assert!(orders_before("1.6.0-12-g2917a80d2", "1.6.0"));
        assert!(orders_before("1.6.0-9-g2917a80d2", "1.6.0-12-g2917a80d2"));
        assert!(orders_before(
            "1.6.0-rc2-156-g2917a80d2",
            "1.6.0-12-g0123abcde"
        ));
        assert!(orders_before("1.6.0", "1.6.1-rc1"));
    }

    #[test]
    fn text_in_no_golem_shape_compares_as_before() {
        assert!(orders_before("1.5.0-alpha", "1.5.0-beta"));
        assert!(orders_before("1.5.1-rc.1", "1.5.1-rc.2"));
        assert!(orders_before("1.5.0", "1.5.1-rc.1"));
        assert!(orders_before("1.5.0-rcx", "1.5.0-rcy"));
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0-12-gnothex"), 2, "1.5.0"),
            Some("1.6.0-12-gnothex".to_string())
        );
    }

    #[test]
    fn the_same_version_behind_moves_nothing() {
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.5.0"), 2, "1.5.0"),
            None
        );
    }

    #[test]
    fn an_older_version_behind_moves_nothing() {
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.4.9"), 2, "1.5.0"),
            None,
            "a rollback queues behind the replicas that are already there"
        );
    }

    #[test]
    fn a_version_that_is_no_semantic_version_never_moves_anything() {
        assert_eq!(
            newer_version_behind(&queue_ending_in(""), 2, "1.5.0"),
            None,
            "a replica that announces no version is never newer"
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in("not a version"), 2, "1.5.0"),
            None
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0"), 2, ""),
            None,
            "a replica that announces no version never moves"
        );
    }

    #[test]
    fn a_development_build_compares_as_a_pre_release_of_the_next_release() {
        // The form `golem_version()` has on a development build.
        let dev = "1.6.0-rc2-156-g2917a80d2";
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0"), 2, dev),
            Some("1.6.0".to_string()),
            "the release is newer than a build after its release candidate"
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in(dev), 2, "1.6.0"),
            None,
            "a build after a release candidate is older than the release"
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0-rc2-157-g0123abcde"), 2, dev),
            Some("1.6.0-rc2-157-g0123abcde".to_string()),
            "a later build of the same tag is newer"
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.5.9"), 2, dev),
            None,
            "an older release is older than a development build"
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in(dev), 2, "1.6.0-rc2"),
            Some(dev.to_string()),
            "a build after a release candidate is newer than that candidate"
        );
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0-rc2"), 2, dev),
            None,
            "the candidate is older than a build after it"
        );
    }

    #[test]
    fn build_metadata_is_ignored() {
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.5.0+b"), 2, "1.5.0+a"),
            None
        );
    }

    #[test]
    fn the_head_of_the_queue_never_moves() {
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0"), 1, "1.5.0"),
            None
        );
    }

    #[test]
    fn a_newer_version_ahead_is_no_reason_to_move() {
        let view = CampaignView {
            campaigners: vec![
                with_version(campaigner(1, 10, 9001), "1.5.0"),
                with_version(campaigner(3, 30, 9003), "1.6.0"),
                with_version(campaigner(2, 40, 9002), "1.5.0"),
            ],
        };
        assert_eq!(newer_version_behind(&view, 2, "1.5.0"), None);
    }

    #[test]
    fn a_view_without_the_own_key_moves_nothing() {
        assert_eq!(
            newer_version_behind(&queue_ending_in("1.6.0"), 9, "1.5.0"),
            None
        );
    }

    #[test]
    fn the_belief_clock_never_goes_back_and_follows_real_time() {
        let before = belief_clock::now();
        std::thread::sleep(Duration::from_millis(30));
        let after = belief_clock::now();
        assert!(
            after >= before + Duration::from_millis(30),
            "{before:?} {after:?}"
        );
    }

    #[test]
    fn a_belief_lapses_when_the_belief_clock_jumps_past_its_deadline() {
        // A suspended machine moves the belief clock without moving the runtime's monotonic clock.
        let belief = LeaseBelief::until(Instant::now() + Duration::from_secs(10));
        let now = belief_clock::now();
        assert!(belief.held());
        assert!(belief.held_at(now + Duration::from_secs(5)));
        assert!(!belief.held_at(now + Duration::from_secs(11)));
    }

    #[test]
    fn a_belief_in_the_past_is_not_held() {
        let belief = LeaseBelief::until(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
        assert!(!belief.held());
    }

    /// Feeds reads of `(seconds since the start, whole-second answer)` to the evidence, and returns
    /// the time of the first read that proves a renewal.
    fn proven_at(reads: &[(f64, i64)]) -> Option<f64> {
        let start = Instant::now();
        let mut evidence = RenewalEvidence::default();
        reads.iter().find_map(|(at, ttl)| {
            let at_instant = start + Duration::from_secs_f64(*at);
            evidence
                .observe(at_instant, at_instant, *ttl, 1)
                .then_some(*at)
        })
    }

    /// Like [`proven_at`], for reads of `(sent, received, answer)`, in seconds since the start.
    fn proven_by_brackets(reads: &[(f64, f64, i64)]) -> Option<f64> {
        let start = Instant::now();
        let mut evidence = RenewalEvidence::default();
        reads.iter().find_map(|(sent, received, ttl)| {
            evidence
                .observe(
                    start + Duration::from_secs_f64(*sent),
                    start + Duration::from_secs_f64(*received),
                    *ttl,
                    1,
                )
                .then_some(*received)
        })
    }

    #[test]
    fn an_implausible_answer_is_ignored_and_cannot_panic() {
        assert!(matches!(
            LeaseRead::of_answer(9_223_372_036, 10, 1),
            LeaseRead::Unknown
        ));
        assert!(matches!(
            LeaseRead::of_answer(i64::MAX, 10, 1),
            LeaseRead::Unknown
        ));
        assert!(matches!(
            LeaseRead::of_answer(12, 10, 1),
            LeaseRead::Alive { remaining: 12, .. }
        ));
        assert!(matches!(
            LeaseRead::of_answer(13, 10, 1),
            LeaseRead::Unknown
        ));

        // Even an answer that reaches `observe` cannot panic or move the evidence.
        let start = Instant::now();
        let mut evidence = RenewalEvidence::default();
        assert!(!evidence.observe(start, start, i64::MAX, 1));
        assert!(!evidence.observe(start, start, 5, 1));
        assert!(!evidence.observe(start, start, i64::MAX, 1));
        assert!(!evidence.observe(
            start + Duration::from_secs(1),
            start + Duration::from_secs(1),
            3,
            1
        ));
    }

    #[test]
    fn an_abandoned_lease_with_a_fractional_end_is_never_counted() {
        // The lease ends 9.7 s after the start; etcd truncates what is left to whole seconds.
        let end = 9.7;
        let reads: Vec<(f64, i64)> = (0..32)
            .map(|i| i as f64 * 0.3)
            .take_while(|at| *at < end)
            .map(|at| (at, (end - at).trunc() as i64))
            .collect();
        assert_eq!(proven_at(&reads), None);
    }

    #[test]
    fn the_bracket_of_a_read_is_its_send_and_its_answer() {
        // The lease ends 9.5 s after the start. Each read takes 0.9 s and etcd answers at its end,
        // so the answer is truncated from what is left at the answer. A bracket with the two
        // instants swapped would count extensions that never happened.
        let end = 9.5;
        let reads: Vec<(f64, f64, i64)> = (0..10)
            .map(|i| i as f64 * 0.2)
            .map(|sent| (sent, sent + 0.9, (end - (sent + 0.9)).trunc() as i64))
            .collect();
        assert_eq!(proven_by_brackets(&reads), None);
    }

    #[test]
    fn a_lease_renewed_to_a_constant_truncated_answer_is_proven_within_a_few_seconds() {
        // A two second lease renewed every third of it always reads one: no answer ever rises.
        let reads: Vec<(f64, i64)> = (0..20).map(|i| (i as f64 * 0.5, 1)).collect();
        let proven = proven_at(&reads).expect("a renewed lease is proven");
        assert!(proven <= 3.0, "{proven}");
    }

    #[test]
    fn a_lease_that_only_counts_down_is_never_counted() {
        let reads: Vec<(f64, i64)> = (0..10).map(|i| (i as f64, 9 - i)).collect();
        assert_eq!(proven_at(&reads), None);
    }

    #[test]
    fn a_countdown_with_one_reset_counts_once() {
        // An etcd leader change grants every lease its full time again, once.
        let reads = [(0.0, 5), (1.0, 4), (2.0, 10), (3.0, 9), (4.0, 8), (5.0, 7)];
        assert_eq!(proven_at(&reads), None);
    }

    #[test]
    fn an_answer_of_zero_past_the_end_is_not_counted() {
        // A lease that has run out and is not yet revoked reads zero.
        assert_eq!(proven_at(&[(0.0, 1), (5.0, 0), (6.0, 0), (7.0, 0)]), None);
    }

    #[test]
    fn a_gap_of_failed_reads_followed_by_a_reset_counts_once() {
        assert_eq!(proven_at(&[(0.0, 5), (10.0, 10), (11.0, 9)]), None);
    }

    #[test]
    fn a_low_read_after_an_extension_does_not_count_twice() {
        // The second read sees an extension; a later read that answers lower, and one after it
        // that answers as high as the extension allows, must not count as more.
        assert_eq!(
            proven_at(&[(0.0, 5), (2.0, 10), (3.0, 3), (3.1, 9), (3.2, 8)]),
            None
        );
    }

    /// Like [`proven_at`], for reads of `(seconds since the start, whole-second answer, raft term)`.
    fn proven_with_terms(reads: &[(f64, i64, u64)]) -> Option<f64> {
        let start = Instant::now();
        let mut evidence = RenewalEvidence::default();
        reads.iter().find_map(|(at, ttl, term)| {
            let at_instant = start + Duration::from_secs_f64(*at);
            evidence
                .observe(at_instant, at_instant, *ttl, *term)
                .then_some(*at)
        })
    }

    #[test]
    fn leader_changes_of_etcd_never_prove_a_renewal_however_many() {
        // An abandoned eight second lease that every new etcd leader extends once.
        assert_eq!(
            proven_with_terms(&[
                (0.0, 7, 1),
                (2.0, 5, 1),
                (3.0, 7, 2),
                (5.0, 5, 2),
                (6.0, 7, 3)
            ]),
            None
        );
        let many: Vec<(f64, i64, u64)> = (0..12)
            .flat_map(|i| {
                let at = i as f64 * 3.0;
                [(at, 4, i as u64 + 1), (at + 1.0, 3, i as u64 + 1)]
            })
            .collect();
        assert_eq!(proven_with_terms(&many), None);
    }

    #[test]
    fn extensions_within_one_term_still_prove_a_renewal() {
        let reads: Vec<(f64, i64, u64)> = (0..20).map(|i| (i as f64 * 0.5, 1, 4)).collect();
        let proven = proven_with_terms(&reads).expect("a renewed lease is proven");
        assert!(proven <= 3.0, "{proven}");
    }

    #[test]
    fn a_term_change_between_two_extensions_of_a_live_renewer_delays_the_proof() {
        // A lease with one second left, renewed all the time, read every half second. It is
        // proven by 3 s in one term; the term changes at 2.5 s, one extension in, and the proof
        // then needs two extensions of the new term.
        let reads: Vec<(f64, i64, u64)> = (0..20)
            .map(|i| i as f64 * 0.5)
            .map(|at| (at, 1, if at < 2.5 { 1 } else { 2 }))
            .collect();
        let proven = proven_with_terms(&reads).expect("proven in the new term");
        assert!(proven >= 5.0, "{proven}");
    }

    #[test]
    fn a_read_without_a_term_changes_nothing() {
        let start = Instant::now();
        let mut evidence = RenewalEvidence::default();
        assert!(!evidence.observe(start, start, 5, 1));
        let later = start + Duration::from_secs(1);
        assert!(!evidence.observe(later, later, 1, 0));
        assert!(!evidence.observe(later, later, 9, 0));
    }

    #[test]
    fn another_campaigner_with_this_address_is_a_duplicate() {
        let view = CampaignView {
            campaigners: vec![campaigner(1, 10, 9001), campaigner(2, 20, 9001)],
        };
        assert_eq!(duplicate_of(&view, 2, &own()).map(|c| c.lease), Some(1));
    }

    #[test]
    fn this_replicas_own_key_is_not_a_duplicate() {
        let view = CampaignView {
            campaigners: vec![campaigner(2, 20, 9001)],
        };
        assert_eq!(duplicate_of(&view, 2, &own()), None);
    }

    #[test]
    fn a_view_without_the_own_key_yet_still_finds_a_duplicate_of_another() {
        let view = CampaignView {
            campaigners: vec![campaigner(1, 10, 9001)],
        };
        assert_eq!(duplicate_of(&view, 2, &own()).map(|c| c.lease), Some(1));
    }

    /// How much later than its send a response is read. Long enough that dating the deadline by
    /// the read instead of the send is unambiguous, short enough to cost nothing.
    const LAG: Duration = Duration::from_millis(20);

    #[test]
    // The lease etcd holds started when the renewal reached it, not when the answer was read.
    async fn the_watchdog_dates_a_renewal_by_when_it_was_sent() {
        let armed_at = SentAt::now();
        let mut watchdog = RenewalWatchdog::armed_at(armed_at, TTL);

        let sent_at = SentAt::now();
        watchdog.record_send(sent_at);
        sleep(LAG).await;
        watchdog.record_acknowledgement();

        let dated_from = watchdog.deadline - TTL;
        assert_eq!(
            dated_from,
            sent_at.monotonic,
            "The deadline was dated {:?} after the renewal was sent, so it is being armed from the \
             moment the response was read rather than from the renewal etcd acknowledged. A \
             `drive()` entry that is dropped leaves its acknowledgement unread until the next one, \
             and the replica would then go on believing in a lease that much past its expiry.",
            dated_from.saturating_duration_since(sent_at.monotonic)
        );
    }

    #[test]
    // etcd answers a stream in order, so the oldest outstanding send is what the next response is
    // for.
    async fn acknowledgements_are_matched_to_sends_in_order() {
        let mut watchdog = RenewalWatchdog::armed_at(SentAt::now(), TTL);

        let first = SentAt::now();
        watchdog.record_send(first);
        sleep(LAG).await;
        let second = SentAt::now();
        watchdog.record_send(second);

        watchdog.record_acknowledgement();
        assert_eq!(
            watchdog.deadline - TTL,
            first.monotonic,
            "The first response was dated to a later send, crediting the lease with a renewal etcd \
             has not answered yet"
        );

        watchdog.record_acknowledgement();
        assert_eq!(
            watchdog.deadline - TTL,
            second.monotonic,
            "The second response did not move the deadline on to the send it answers"
        );
    }

    #[test]
    // The keepalive handshake answers with a positive TTL too, and that response has no send of
    // its own to date it against.
    async fn a_response_with_nothing_to_date_it_leaves_the_deadline_alone() {
        let armed_at = SentAt::now();
        let mut watchdog = RenewalWatchdog::armed_at(armed_at, TTL);

        sleep(LAG).await;
        watchdog.record_acknowledgement();

        assert_eq!(
            watchdog.deadline - TTL,
            armed_at.monotonic,
            "A response this replica cannot place in time moved the deadline, extending the belief \
             window on the strength of a renewal it never sent"
        );
    }

    #[test]
    // A reconnect replaces the stream; nothing sent on its predecessor can be answered on it.
    async fn a_replaced_stream_does_not_date_its_first_response_to_an_old_send() {
        let mut watchdog = RenewalWatchdog::armed_at(SentAt::now(), TTL);

        let on_the_old_stream = SentAt::now();
        watchdog.record_send(on_the_old_stream);
        watchdog.forget_unacknowledged();
        sleep(LAG).await;
        let on_the_new_stream = SentAt::now();
        watchdog.record_send(on_the_new_stream);
        watchdog.record_acknowledgement();

        assert_eq!(
            watchdog.deadline - TTL,
            on_the_new_stream.monotonic,
            "The new stream's first response was dated to a send made on the stream it replaced, \
             so every reconnect shortens the lease this replica believes it holds"
        );
    }

    #[test]
    // A suspend between the send and the answer is time etcd's lease has run: the belief is dated
    // from the send on the belief clock, not from the moment the answer is read.
    async fn the_belief_is_dated_by_the_belief_clock_at_the_send() {
        // The belief clock reads 100 s at the send, whatever it reads when the answer arrives: a
        // suspend in between moves the later reading, not the send.
        let send = Duration::from_secs(100);
        let sent = SentAt {
            monotonic: Instant::now(),
            belief: send,
        };
        let mut watchdog = RenewalWatchdog::armed_at(SentAt::now(), TTL);
        watchdog.record_send(sent);
        watchdog.record_acknowledgement();

        assert!(
            watchdog
                .belief
                .held_at(send + TTL - Duration::from_millis(1))
        );
        assert!(!watchdog.belief.held_at(send + TTL));
    }
}
