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
use crate::sharding::error::ShardManagerError;
use crate::sharding::etcd_connection::connect_for_election;
use crate::sharding::etcd_retry::{RETRY_MAX, RETRY_MIN, retry_retriable_until};
use etcd_client::{
    Client, Compare, CompareOp, LeaderKey, LeaseClient, LeaseKeepAliveStream, LeaseKeeper,
};
use golem_common::retriable_error::IsRetriableError;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep, sleep_until, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    unacknowledged: VecDeque<Instant>,
}

impl RenewalWatchdog {
    /// Starts the belief window at `sent_at`: the instant the handshake was sent, which etcd has
    /// since answered.
    fn armed_at(sent_at: Instant, ttl: Duration) -> Self {
        Self {
            ttl,
            deadline: sent_at + ttl,
            unacknowledged: VecDeque::new(),
        }
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn record_send(&mut self, sent_at: Instant) {
        self.unacknowledged.push_back(sent_at);
    }

    /// Re-arms from the send this response answers, never from when it was read:
    /// [`LeaseKeepAlive::drive`] is entered more than once, and an entry that is dropped leaves its
    /// acknowledgement unread until the next one. A response with no send behind it is the
    /// handshake's own, which already armed the deadline.
    fn record_acknowledgement(&mut self) {
        if let Some(sent_at) = self.unacknowledged.pop_front() {
            self.deadline = sent_at + self.ttl;
        }
    }

    /// Drops the sends still outstanding on a replaced stream: nothing sent on it can be answered
    /// by its successor.
    fn forget_unacknowledged(&mut self) {
        self.unacknowledged.clear();
    }

    /// Re-arms from a renewal acknowledged outside the response stream - the one `keep_alive`
    /// performs when a reconnect succeeds.
    fn rearm_at(&mut self, sent_at: Instant) {
        self.deadline = sent_at + self.ttl;
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
                    let sent_at = Instant::now();
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
            let attempt_sent = Instant::now();
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
}

impl LeadershipHandle {
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

pub struct LeaderElection {
    client: Client,
    election_name: String,
    lease_ttl: Duration,
    request_timeout: Duration,
    standby_log_interval: Duration,
    identity: String,
    shutdown: CancellationToken,
    confirm_hook: Option<ConfirmHook>,
    /// Counts every confirming read this election has made, across campaigns, so that the hook can
    /// name one attempt out of the whole run rather than one per campaign.
    confirm_attempts: AtomicU32,
    /// The lease of the attempt in flight, or 0. Set after the grant and cleared when the attempt
    /// finishes, so a caller that drops the campaign can still revoke what it left behind.
    pending_lease: AtomicI64,
}

impl LeaderElection {
    pub async fn connect(
        config: &EtcdConfig,
        election_name: impl Into<String>,
    ) -> Result<Self, ShardManagerError> {
        Ok(Self {
            client: connect_for_election(config).await?,
            election_name: election_name.into(),
            lease_ttl: config.leader_lease_ttl,
            request_timeout: config.request_timeout,
            standby_log_interval: STANDBY_LOG_INTERVAL,
            identity: identity(),
            shutdown: CancellationToken::new(),
            confirm_hook: None,
            confirm_attempts: AtomicU32::new(0),
            pending_lease: AtomicI64::new(0),
        })
    }

    /// Ends the campaign when `shutdown` is cancelled. A standby killed while queued leaves its key
    /// in etcd's FIFO for a full TTL, and the next election waits behind it even after a clean
    /// handover.
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
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
            identity = self.identity,
            lease_ttl = ?self.lease_ttl,
            "Campaigning for shard manager leadership"
        );

        self.campaign_loop().await
    }

    async fn campaign_loop(&self) -> Result<Elected, ShardManagerError> {
        let mut backoff = RETRY_MIN;

        loop {
            if self.shutdown.is_cancelled() {
                return Err(ShardManagerError::ShutdownRequested);
            }

            match self.campaign_once().await {
                Ok(elected) => return Ok(elected),
                Err(err) if err.is_retriable() => {
                    metrics::record_campaign_attempt_failure();
                    warn!(error = %err, retry_in = ?backoff, "Campaign attempt failed; retrying");
                    tokio::select! {
                        _ = sleep_until(Instant::now() + backoff) => {}
                        _ = self.shutdown.cancelled() => {
                            return Err(ShardManagerError::ShutdownRequested);
                        }
                    }
                    backoff = std::cmp::min(backoff * 2, RETRY_MAX);
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn campaign_once(&self) -> Result<Elected, ShardManagerError> {
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
    ) -> Result<Elected, ShardManagerError> {
        let handshake_sent = Instant::now();
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
            election_client.campaign(self.election_name.clone(), self.identity.clone(), lease_id);

        // The keepalive runs *during* the campaign: etcd's election server orphans the session it
        // builds from this lease, so nothing renews it server-side, and a standby that waited would
        // watch its own lease expire mid-wait. The announcement is an arm rather than awaited in an
        // arm body so it can never delay a renewal or the campaign.
        let response = tokio::select! {
            biased;
            lost = keepalive.drive() => {
                return Err(ShardManagerError::LeaseLostWhileCampaigning(lost));
            }
            won = campaign => won?,
            _ = self.shutdown.cancelled() => return Err(ShardManagerError::ShutdownRequested),
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
                return Err(ShardManagerError::LeaseLostWhileCampaigning(lost));
            }
            // Leaves through `campaign_once`'s revoke like every other error, so a shutdown here
            // releases the lease the win is holding rather than waiting the confirm out.
            _ = self.shutdown.cancelled() => return Err(ShardManagerError::ShutdownRequested),
            confirmed = self.confirm_fence_is_live(&fence, lease_id, confirm_budget) => confirmed?,
        }

        info!(
            leader_key = fence.key_str(),
            create_revision = fence.create_revision(),
            lease_id,
            granted_ttl = ?granted_ttl,
            identity = self.identity,
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
        let deadline = Instant::now() + budget;
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
                Ok(Some(leader)) => {
                    info!(leader, "Still standing by; leadership is held elsewhere")
                }
                Ok(None) => info!("Still standing by; no leader is currently recorded"),
                Err(err) => warn!(error = %err, "Still standing by; cannot read the leader"),
            }
        }
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

    /// The value recorded by whichever replica currently holds the leadership, for diagnostics.
    pub async fn current_leader(&self) -> Result<Option<String>, ShardManagerError> {
        let mut election_client = self.client.election_client();
        let response = match self
            .bounded(election_client.leader(self.election_name.clone()))
            .await
        {
            Ok(response) => response,
            // etcd reports a leaderless election as an error with the same `Unknown` code as a
            // transport fault; the message is the only thing that tells the two apart.
            Err(err) if is_no_leader(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        Ok(response
            .kv()
            .map(|kv| String::from_utf8_lossy(kv.value()).into_owned()))
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

async fn bounded<T, F>(request_timeout: Duration, request: F) -> Result<T, ShardManagerError>
where
    F: Future<Output = Result<T, etcd_client::Error>>,
{
    timeout(request_timeout, request)
        .await
        .map_err(|_| ShardManagerError::Timeout)?
        .map_err(ShardManagerError::from)
}

/// The value under this replica's election key, diagnostic only. The pid and start time answer
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

    /// How much later than its send a response is read. Long enough that dating the deadline by
    /// the read instead of the send is unambiguous, short enough to cost nothing.
    const LAG: Duration = Duration::from_millis(20);

    #[test]
    // The lease etcd holds started when the renewal reached it, not when the answer was read.
    async fn the_watchdog_dates_a_renewal_by_when_it_was_sent() {
        let armed_at = Instant::now();
        let mut watchdog = RenewalWatchdog::armed_at(armed_at, TTL);

        let sent_at = Instant::now();
        watchdog.record_send(sent_at);
        sleep(LAG).await;
        watchdog.record_acknowledgement();

        let dated_from = watchdog.deadline - TTL;
        assert_eq!(
            dated_from,
            sent_at,
            "The deadline was dated {:?} after the renewal was sent, so it is being armed from the \
             moment the response was read rather than from the renewal etcd acknowledged. A \
             `drive()` entry that is dropped leaves its acknowledgement unread until the next one, \
             and the replica would then go on believing in a lease that much past its expiry.",
            dated_from.saturating_duration_since(sent_at)
        );
    }

    #[test]
    // etcd answers a stream in order, so the oldest outstanding send is what the next response is
    // for.
    async fn acknowledgements_are_matched_to_sends_in_order() {
        let mut watchdog = RenewalWatchdog::armed_at(Instant::now(), TTL);

        let first = Instant::now();
        watchdog.record_send(first);
        sleep(LAG).await;
        let second = Instant::now();
        watchdog.record_send(second);

        watchdog.record_acknowledgement();
        assert_eq!(
            watchdog.deadline - TTL,
            first,
            "The first response was dated to a later send, crediting the lease with a renewal etcd \
             has not answered yet"
        );

        watchdog.record_acknowledgement();
        assert_eq!(
            watchdog.deadline - TTL,
            second,
            "The second response did not move the deadline on to the send it answers"
        );
    }

    #[test]
    // The keepalive handshake answers with a positive TTL too, and that response has no send of
    // its own to date it against.
    async fn a_response_with_nothing_to_date_it_leaves_the_deadline_alone() {
        let armed_at = Instant::now();
        let mut watchdog = RenewalWatchdog::armed_at(armed_at, TTL);

        sleep(LAG).await;
        watchdog.record_acknowledgement();

        assert_eq!(
            watchdog.deadline - TTL,
            armed_at,
            "A response this replica cannot place in time moved the deadline, extending the belief \
             window on the strength of a renewal it never sent"
        );
    }

    #[test]
    // A reconnect replaces the stream; nothing sent on its predecessor can be answered on it.
    async fn a_replaced_stream_does_not_date_its_first_response_to_an_old_send() {
        let mut watchdog = RenewalWatchdog::armed_at(Instant::now(), TTL);

        let on_the_old_stream = Instant::now();
        watchdog.record_send(on_the_old_stream);
        watchdog.forget_unacknowledged();
        sleep(LAG).await;
        let on_the_new_stream = Instant::now();
        watchdog.record_send(on_the_new_stream);
        watchdog.record_acknowledgement();

        assert_eq!(
            watchdog.deadline - TTL,
            on_the_new_stream,
            "The new stream's first response was dated to a send made on the stream it replaced, \
             so every reconnect shortens the lease this replica believes it holds"
        );
    }
}
