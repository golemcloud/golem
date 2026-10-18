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

//! Sends shard manager calls to the leader. Every replica accepts the call, but only the leader
//! serves it; a standby answers with the leader's address, and the router connects there and keeps
//! that connection until a call to it fails.

use crate::clients::shard_manager::GrpcShardManagerConfig;
use crate::clients::shard_manager_leader::{LeaderAddress, is_not_leader, leader_of};
use crate::grpc::client::{
    EnabledGrpcClientTlsConfig, GrpcClient, GrpcClientConfig, GrpcClientTlsConfig,
    MultiTargetGrpcClient, peer_answered_unavailable, requires_reconnect,
};
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;
use golem_common::model::RetryConfig;
use golem_common::retries::get_delay;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::{Code, Status};
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;
use tracing::{debug, info};

/// How long the client waits to connect to a leader that a replica named. Shorter than the
/// configured `connect_timeout`: a named leader that does not answer quickly is usually gone, and
/// the configured address can name its successor.
const LEADER_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How many leaders that an answer named one call follows in one pass. Replicas name each other
/// only while a leadership change is being observed, so a chain is short; the limit ends a loop
/// of replicas that disagree.
const MAX_REDIRECT_HOPS: usize = 3;

/// How long a call that may be repeated keeps asking once the replicas have shown a failover. The
/// shard manager's default `leader_lease_ttl` is 10 s, and until it passes the replicas keep naming
/// the dead leader; the new leader then starts up, which skips the executor check while the leases
/// it reads are valid and otherwise waits at most 3 s. Together that is about 13 s, rounded up.
const FAILOVER_BUDGET: Duration = Duration::from_secs(15);

/// The backoff between passes while a failover is being waited out: it starts quickly, because a
/// new leader may be named at any moment, and settles at the longest pause that still notices one
/// within half a second.
fn failover_backoff() -> RetryConfig {
    RetryConfig {
        max_attempts: u32::MAX,
        min_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        multiplier: 2.0,
        max_jitter_factor: None,
    }
}

/// The time that the configured `retries_on_unavailable` schedule takes in all, without jitter.
fn schedule_total(schedule: &RetryConfig) -> Duration {
    // The delays are those of `get_delay`, which counts a fractional multiplier as its integer
    // part. The rest of a very long schedule is counted at `max_delay` instead of one attempt at a
    // time, which overcounts it only if the delays have not reached `max_delay` by then.
    const COUNTED_ONE_BY_ONE: u32 = 10_000;
    let schedule = RetryConfig {
        max_jitter_factor: None,
        ..schedule.clone()
    };
    let counted = schedule.max_attempts.min(COUNTED_ONE_BY_ONE);
    let mut total = Duration::ZERO;
    for attempt in 1..counted {
        total = total.saturating_add(get_delay(&schedule, attempt).unwrap_or_default());
    }
    total.saturating_add(
        schedule
            .max_delay
            .saturating_mul(schedule.max_attempts.saturating_sub(counted)),
    )
}

pub(crate) type Client = ShardManagerServiceClient<OtelGrpcService<Channel>>;

/// One call on a shard manager client, in the form the gRPC clients take it. Written once here:
/// [`LeaderRouter::call`] spells the bound out, because a closure's higher-ranked signature is
/// inferred only from an `Fn` bound written on the parameter itself.
pub(crate) trait ClientCall<R>:
    for<'a> Fn(&'a mut Client) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
    + Send
    + Sync
{
}

impl<R, F> ClientCall<R> for F where
    F: for<'a> Fn(&'a mut Client) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
        + Send
        + Sync
{
}

fn new_client(channel: OtelGrpcService<Channel>, max_message_size: usize) -> Client {
    ShardManagerServiceClient::new(channel)
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(max_message_size)
        .max_encoding_message_size(max_message_size)
}

/// One attempt with a short connect: on any failure the client goes back to the configured
/// address, whose replicas know the current leader.
fn leader_client_config(config: &GrpcShardManagerConfig) -> GrpcClientConfig {
    let client = &config.client_config;
    GrpcClientConfig {
        connect_timeout: client.connect_timeout.min(LEADER_CONNECT_TIMEOUT),
        retries_on_unavailable: RetryConfig {
            max_attempts: 0,
            ..client.retries_on_unavailable.clone()
        },
        tls: leader_tls(&client.tls, &config.host),
        ..client.clone()
    }
}

/// One attempt with a short connect to the configured address: during a failover the router paces
/// the passes itself, and a pass that ran the configured schedule again would last many times the
/// time that a pass is meant to take.
fn once_client_config(config: &GrpcShardManagerConfig) -> GrpcClientConfig {
    let client = &config.client_config;
    GrpcClientConfig {
        connect_timeout: client.connect_timeout.min(LEADER_CONNECT_TIMEOUT),
        retries_on_unavailable: RetryConfig {
            max_attempts: 0,
            ..client.retries_on_unavailable.clone()
        },
        ..client.clone()
    }
}

/// The client reaches the leader by its advertised host, usually a pod IP that the shard managers'
/// certificate does not name. Without a configured server name, the leader's certificate is
/// checked against the configured host instead: the name that clients reach the shard managers
/// by, which their certificate is issued for. An IP literal as the configured host is left alone,
/// because it names one replica, not all of them.
fn leader_tls(tls: &GrpcClientTlsConfig, configured_host: &str) -> GrpcClientTlsConfig {
    let is_ip_literal = configured_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok();
    match tls {
        GrpcClientTlsConfig::Enabled(enabled)
            if enabled.server_domain_name.is_none() && !is_ip_literal =>
        {
            GrpcClientTlsConfig::Enabled(EnabledGrpcClientTlsConfig {
                server_domain_name: Some(configured_host.to_string()),
                ..enabled.clone()
            })
        }
        other => other.clone(),
    }
}

/// A leader that has been logged at info level, and whether its loss has been as well.
struct ReportedLeader {
    address: LeaderAddress,
    forgotten: bool,
}

#[derive(Default)]
struct LeaderState {
    /// The leader that the last answer named, until a call to it fails.
    leader: Option<LeaderAddress>,
    /// The leader last reported at info level, so that a failover, which forgets and learns the
    /// same leader again on every pass, logs one line per change instead of one per pass.
    reported: Option<ReportedLeader>,
}

/// `f`, noting in `answered_unavailable` whether the call it made ended in `UNAVAILABLE` from a
/// replica that was reached. The signature is spelled out so that the closure is inferred as
/// higher-ranked, as in [`LeaderRouter::call`].
fn observe_answers<'f, F, R>(
    f: &'f F,
    answered_unavailable: Arc<AtomicBool>,
) -> impl for<'a> Fn(&'a mut Client) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
+ Send
+ Sync
+ 'f
where
    F: ClientCall<R>,
    R: 'static,
{
    move |client: &mut Client| {
        let call = f(client);
        let answered_unavailable = answered_unavailable.clone();
        Box::pin(async move {
            let result = call.await;
            if let Err(status) = &result {
                answered_unavailable.store(peer_answered_unavailable(status), Ordering::Relaxed);
            }
            result
        })
    }
}

/// The outcome of one pass of [`LeaderRouter::call`].
enum Routed<R> {
    Done(Result<R, Status>),
    /// No leader could be reached in this pass. `failed` is the named leader that this pass tried
    /// and that failed; the next pass skips it, and the pass after that tries it again.
    LeaderUnreachable {
        status: Status,
        failed: Option<LeaderAddress>,
    },
}

/// Whether the known leader cannot serve this client any more: its connection failed, it answered
/// `UNAVAILABLE`, or the process at its address is not a shard manager that accepts this client (a
/// pod IP reused by another service). A request that ran out of its own time, a `DeadlineExceeded`
/// or an application `Unknown` is an answer from a working leader, which stays the one to call.
fn leader_failed(status: &Status) -> bool {
    requires_reconnect(status)
        || matches!(
            status.code(),
            Code::Unimplemented | Code::PermissionDenied | Code::Unauthenticated
        )
}

/// Whether a call may be sent again after a failure that the peer may have seen.
#[derive(Clone, Copy)]
pub(crate) enum Attempts {
    Retried,
    Once,
}

impl Attempts {
    fn replayable(self) -> bool {
        matches!(self, Attempts::Retried)
    }
}

#[derive(Clone)]
pub(crate) struct LeaderRouter {
    /// The configured address, which reaches any replica.
    configured: GrpcClient<Client>,
    /// The configured address with no retries of its own and a short connect, for the passes of a
    /// failover, whose pace the router sets.
    configured_once: GrpcClient<Client>,
    /// Direct connections to the leader that a replica named.
    leaders: MultiTargetGrpcClient<Client>,
    state: Arc<Mutex<LeaderState>>,
    tls: bool,
    /// How long the configured `retries_on_unavailable` schedule takes in all.
    configured_budget: Duration,
}

impl LeaderRouter {
    pub(crate) fn new(config: &GrpcShardManagerConfig) -> Self {
        Self {
            configured: GrpcClient::new(
                "shard_manager",
                new_client,
                config.uri(),
                config.client_config.clone(),
            )
            .with_redirect(is_not_leader),
            configured_once: GrpcClient::new(
                "shard_manager",
                new_client,
                config.uri(),
                once_client_config(config),
            )
            .with_redirect(is_not_leader),
            leaders: MultiTargetGrpcClient::new(
                "shard_manager_leader",
                new_client,
                leader_client_config(config),
            )
            .with_redirect(is_not_leader),
            state: Arc::new(Mutex::new(LeaderState::default())),
            tls: config.client_config.tls_enabled(),
            configured_budget: schedule_total(&config.client_config.retries_on_unavailable),
        }
    }

    /// Sends one call to the leader: to the one already known, otherwise to the configured
    /// address, following a replica that names the leader.
    ///
    /// A replica that names the leader has not run the call, so following it is safe even for a
    /// call that must not be repeated. A failed call to a known leader is not repeated for such a
    /// call, because it may have run; the caller's own schedule tries again.
    ///
    /// After an ungraceful stop, the replicas name the dead leader until its etcd lease expires,
    /// and then know no leader until the new one is ready. A call that may be repeated waits that
    /// out: once a named leader cannot be reached, or a replica answers `UNAVAILABLE` itself, it
    /// keeps asking until [`FAILOVER_BUDGET`] or the time that the configured
    /// `retries_on_unavailable` schedule takes in all has passed since the call began, whichever
    /// is longer. From then on the signal holds for the whole call, and a pass sends the call to
    /// the configured address once instead of running its schedule again. A shard manager that
    /// cannot be reached at all is no failover and keeps the configured retries.
    pub(crate) async fn call<F, R>(
        &self,
        description: &'static str,
        attempts: Attempts,
        f: F,
    ) -> Result<R, Status>
    where
        R: 'static,
        F: for<'a> Fn(
                &'a mut Client,
            ) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send
            + Sync,
    {
        let mut failed_last_pass = None;
        let failover_backoff = failover_backoff();
        let started = Instant::now();
        // Fixed when the first failover signal is seen, and the end of the wait from then on.
        let mut deadline: Option<Instant> = None;
        let mut failover_passes = 0;
        // Whether the last answer a replica gave this call was its own `UNAVAILABLE`, as opposed
        // to a failure to reach one.
        let answered_unavailable = Arc::new(AtomicBool::new(false));
        let observing = observe_answers(&f, answered_unavailable.clone());
        loop {
            // A named leader that failed is skipped for one pass only. Its failure may have been
            // brief, and skipping it for the rest of the call would fail a call that it could
            // still serve.
            let skip = failed_last_pass.take();
            answered_unavailable.store(false, Ordering::Relaxed);
            let routed = self
                .route_once(
                    description,
                    attempts,
                    deadline.is_some(),
                    &observing,
                    skip.as_ref(),
                )
                .await;
            let (status, failed) = match routed {
                Routed::Done(Ok(result)) => return Ok(result),
                // The configured address answered `UNAVAILABLE` itself: a replica that works but
                // knows no leader to name, which is how a failover looks before the new leader is
                // ready. Anything else from it, such as a failure to connect, is the shard manager
                // being unreachable and keeps the configured retries, unless a failover has been
                // seen already: then the address may be a service that still routes to the
                // replica that went away, and the wait goes on.
                Routed::Done(Err(status))
                    if attempts.replayable()
                        && if deadline.is_some() {
                            requires_reconnect(&status)
                        } else {
                            peer_answered_unavailable(&status)
                                && answered_unavailable.load(Ordering::Relaxed)
                        } =>
                {
                    (status, None)
                }
                Routed::Done(Err(status)) => return Err(status),
                Routed::LeaderUnreachable { status, failed } => (status, failed),
            };
            failed_last_pass = failed;
            if !attempts.replayable() {
                return Err(status);
            }
            let deadline = *deadline.get_or_insert_with(|| {
                let budget = FAILOVER_BUDGET.max(self.configured_budget);
                // A schedule so long that the sum overflows is as good as one without an end.
                started
                    .checked_add(budget)
                    .or_else(|| started.checked_add(Duration::from_secs(10 * 365 * 24 * 3600)))
                    .unwrap_or(started)
            });
            failover_passes += 1;
            let remaining = deadline.saturating_duration_since(Instant::now());
            match get_delay(&failover_backoff, failover_passes) {
                Some(delay) if !remaining.is_zero() => {
                    tokio::time::sleep(delay.min(remaining)).await;
                }
                _ => return Err(status),
            }
        }
    }

    /// One pass of [`Self::call`]. `skip` is the named leader that the previous pass could not
    /// reach; while the replicas still name it, this pass ends without trying it again. In a
    /// `failover` the configured address is asked once per pass; the pace is the caller's.
    async fn route_once<F, R>(
        &self,
        description: &'static str,
        attempts: Attempts,
        failover: bool,
        f: &F,
        skip: Option<&LeaderAddress>,
    ) -> Routed<R>
    where
        F: ClientCall<R>,
    {
        let mut failed_here = None;
        if let Some(leader) = self.known_leader() {
            match self.send_to_leader(description, &leader, attempts, f).await {
                Ok(result) => return Routed::Done(Ok(result)),
                Err(status) => {
                    if let Some(next) = leader_of(&status) {
                        self.forget_leader(&leader, &status);
                        if attempts.replayable() && skip == Some(&next) {
                            return Routed::LeaderUnreachable {
                                status,
                                failed: None,
                            };
                        }
                        return self.follow(description, next, attempts, skip, f).await;
                    }
                    // The shard manager answers most refusals inside a successful response, but
                    // some with a status such as `InvalidArgument`: those come from a working
                    // leader, which stays the one to call.
                    if !leader_failed(&status) {
                        return Routed::Done(Err(status));
                    }
                    self.forget_leader(&leader, &status);
                    if !attempts.replayable() {
                        return Routed::Done(Err(status));
                    }
                    failed_here = Some(leader);
                }
            }
        }

        let status = match self
            .send_to_configured(description, attempts, failover, f)
            .await
        {
            Ok(result) => return Routed::Done(Ok(result)),
            Err(status) => status,
        };
        match leader_of(&status) {
            // Named until that leader's lease expires; wait, then ask again.
            Some(next) if skip == Some(&next) || failed_here.as_ref() == Some(&next) => {
                Routed::LeaderUnreachable {
                    status,
                    failed: failed_here,
                }
            }
            Some(next) => self.follow(description, next, attempts, skip, f).await,
            // The configured address has already spent its own retries.
            None => Routed::Done(Err(status)),
        }
    }

    /// Sends the call to a leader that an answer named, and on to the next one if that replica
    /// names another. A replica that names a leader has not run the call, so this holds for every
    /// call. At most [`MAX_REDIRECT_HOPS`] named leaders are tried in one pass; the last answer
    /// then counts as a failed follow, which ends a loop of replicas that name each other. A
    /// leader that `skip` names failed in the previous pass and is not tried in this one, wherever
    /// in the chain it is named.
    async fn follow<F, R>(
        &self,
        description: &'static str,
        mut leader: LeaderAddress,
        attempts: Attempts,
        skip: Option<&LeaderAddress>,
        f: &F,
    ) -> Routed<R>
    where
        F: ClientCall<R>,
    {
        let mut hops = 0;
        loop {
            hops += 1;
            // Two calls redirected by different replicas can race here; a stale leader that wins
            // is forgotten at its first failure.
            self.remember_leader(leader.clone());
            match self.send_to_leader(description, &leader, attempts, f).await {
                Ok(result) => return Routed::Done(Ok(result)),
                Err(status) => {
                    let named = leader_of(&status);
                    if named.is_none() && !leader_failed(&status) {
                        return Routed::Done(Err(status));
                    }
                    self.forget_leader(&leader, &status);
                    match named {
                        Some(next) if attempts.replayable() && skip == Some(&next) => {
                            return Routed::LeaderUnreachable {
                                status,
                                failed: None,
                            };
                        }
                        Some(next) if hops < MAX_REDIRECT_HOPS => leader = next,
                        _ => {
                            if !attempts.replayable() {
                                return Routed::Done(Err(status));
                            }
                            return Routed::LeaderUnreachable {
                                status,
                                failed: Some(leader),
                            };
                        }
                    }
                }
            }
        }
    }

    async fn send_to_leader<F, R>(
        &self,
        description: &'static str,
        leader: &LeaderAddress,
        attempts: Attempts,
        f: &F,
    ) -> Result<R, Status>
    where
        F: ClientCall<R>,
    {
        self.leaders
            .call_with_retry(description, leader.uri(self.tls), attempts.replayable(), f)
            .await
    }

    async fn send_to_configured<F, R>(
        &self,
        description: &'static str,
        attempts: Attempts,
        failover: bool,
        f: &F,
    ) -> Result<R, Status>
    where
        F: ClientCall<R>,
    {
        let client = if failover {
            &self.configured_once
        } else {
            &self.configured
        };
        client
            .call_with_retry(description, attempts.replayable(), f)
            .await
    }

    fn known_leader(&self) -> Option<LeaderAddress> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .leader
            .clone()
    }

    fn remember_leader(&self, leader: LeaderAddress) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.reported.as_ref().map(|r| &r.address) != Some(&leader) {
            info!(leader = %leader, "Calling a newly named shard manager leader");
            state.reported = Some(ReportedLeader {
                address: leader.clone(),
                forgotten: false,
            });
        }
        state.leader = Some(leader);
    }

    /// Only while it is still the one that failed: a concurrent call may have learned a newer one.
    fn forget_leader(&self, leader: &LeaderAddress, cause: &Status) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.leader.as_ref() == Some(leader) {
            // A failover forgets the same leader on every pass; only the first loss is news.
            let first_loss = match state.reported.as_mut() {
                Some(r) if &r.address == leader && !r.forgotten => {
                    r.forgotten = true;
                    true
                }
                _ => false,
            };
            if first_loss {
                info!(
                    leader = %leader,
                    code = ?cause.code(),
                    error = cause.message(),
                    "No longer calling this shard manager leader"
                );
            } else {
                debug!(
                    leader = %leader,
                    code = ?cause.code(),
                    error = cause.message(),
                    "No longer calling this shard manager leader"
                );
            }
            state.leader = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::base64::Base64;
    use test_r::test;

    fn enabled(server_domain_name: Option<&str>) -> GrpcClientTlsConfig {
        GrpcClientTlsConfig::Enabled(EnabledGrpcClientTlsConfig {
            client_cert: Base64(vec![]),
            client_key: Base64(vec![]),
            server_ca_cert: Base64(vec![]),
            server_domain_name: server_domain_name.map(str::to_string),
        })
    }

    fn name(tls: &GrpcClientTlsConfig) -> Option<String> {
        match tls {
            GrpcClientTlsConfig::Enabled(enabled) => enabled.server_domain_name.clone(),
            GrpcClientTlsConfig::Disabled(_) => None,
        }
    }

    #[test]
    fn the_schedule_total_adds_the_delays_between_attempts() {
        let schedule = RetryConfig {
            max_attempts: 4,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
            multiplier: 2.0,
            max_jitter_factor: Some(0.5),
        };
        // 100 ms, 200 ms and the third delay capped at 250 ms.
        assert_eq!(schedule_total(&schedule), Duration::from_millis(550));
        assert_eq!(
            schedule_total(&RetryConfig {
                max_attempts: 0,
                ..schedule.clone()
            }),
            Duration::ZERO
        );
    }

    #[test]
    fn a_very_long_schedule_is_counted_at_its_longest_delay() {
        let schedule = RetryConfig {
            max_attempts: 20_000,
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(10),
            multiplier: 1.0,
            max_jitter_factor: None,
        };
        assert_eq!(schedule_total(&schedule), Duration::from_millis(199_990));
    }

    #[test]
    fn the_leader_is_checked_against_a_configured_dns_name() {
        assert_eq!(
            name(&leader_tls(&enabled(None), "golem-shard-manager")).as_deref(),
            Some("golem-shard-manager")
        );
    }

    #[test]
    fn a_configured_server_name_wins() {
        assert_eq!(
            name(&leader_tls(
                &enabled(Some("sm.example")),
                "golem-shard-manager"
            ))
            .as_deref(),
            Some("sm.example")
        );
    }

    #[test]
    fn an_ip_literal_host_is_not_used_as_a_server_name() {
        assert_eq!(name(&leader_tls(&enabled(None), "10.0.0.7")), None);
        assert_eq!(name(&leader_tls(&enabled(None), "[::1]")), None);
    }
}
