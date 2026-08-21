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

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::SafeDisplay;
use golem_common::model::base64::Base64;
use golem_common::model::{Empty, RetryConfig};
use golem_common::retries::RetryState;
use http::Uri;
use scc::hash_map::Entry;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};
use tonic_tracing_opentelemetry::middleware::client::{OtelGrpcLayer, OtelGrpcService};
use tower::ServiceBuilder;
use tracing::{Instrument, debug, debug_span, warn};

use crate::metrics::grpc::{
    record_internal_grpc_failure, record_internal_grpc_retry, record_internal_grpc_success,
};

fn build_endpoint(
    uri: Uri,
    config: &GrpcClientConfig,
) -> Result<Endpoint, tonic::transport::Error> {
    let mut endpoint = Endpoint::new(uri)?.connect_timeout(config.connect_timeout);

    if let Some(request_timeout) = config.request_timeout {
        endpoint = endpoint.timeout(request_timeout);
    }

    if let Some(interval) = config.http2_keep_alive_interval {
        endpoint = endpoint.http2_keep_alive_interval(interval);
    }

    if let Some(timeout) = config.http2_keep_alive_timeout {
        endpoint = endpoint.keep_alive_timeout(timeout);
    }

    if let Some(while_idle) = config.http2_keep_alive_while_idle {
        endpoint = endpoint.keep_alive_while_idle(while_idle);
    }

    if let GrpcClientTlsConfig::Enabled(tls) = &config.tls {
        endpoint = endpoint.tls_config(tls.to_tonic())?;
    }

    Ok(endpoint)
}

/// A connection attempt shared by every caller waiting for the same target.
///
/// Without this, callers queue inside tonic's `Channel` and each waits for the
/// one ahead of it to burn a full `connect_timeout`. Sharing the attempt means an
/// unreachable peer costs the whole cohort one `connect_timeout` between them,
/// after which they all fail and the caller can re-resolve.
///
/// What it resolves to is the client, not the channel underneath it. There is
/// one cache, so nothing can hold a second opinion about which connection to a
/// target is the current one.
type SharedConnect<T> = Shared<BoxFuture<'static, Result<GrpcClientConnection<T>, Status>>>;

/// Builds the client a connection is used through.
type ClientFactory<T> = Arc<dyn Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync>;

/// How long a connection to a target nobody is calling is kept.
///
/// Executor pods come and go, and a target that is never called again would
/// otherwise hold its `Channel` — and the `Buffer` worker task tower spawned for
/// it — for the life of the process. Reaching a dropped target again costs one
/// reconnect, which is why this is generous rather than tight.
///
/// A client with one fixed target uses `Duration::MAX` instead: it holds one
/// connection to a service that is meant to be there, and holding on to it is
/// the point.
const IDLE_CONNECTION_TTL: Duration = Duration::from_secs(600);

/// How often connections to targets nobody is calling are dropped.
///
/// Every call checks whether this much time has passed, and the first to find
/// that it has does the work. Hanging the sweep off a cache miss instead, as
/// this once did, meant it stopped running altogether once every target in a
/// stable cluster was connected — which is precisely when there is something to
/// drop, because a target that goes quiet produces no misses to sweep on.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// How many connections a caller will establish when each is retired before it
/// can be handed over.
///
/// A retirement landing in that window is a sibling's failure, not this
/// caller's, and the connection it takes out is forgotten as it goes — so going
/// back for another gets a fresh one rather than the same verdict.
///
/// Three because what a caller sees decays geometrically with each turn and
/// never flattens, while what it costs saturates after the second. Measured
/// against a peer retiring one call in ten, with 16 callers sharing it: going
/// back once takes callers failing outright from 9.5% to 1.5%, twice takes it
/// to 0.25% and a third time to 0.03%, for 6%, 9% and 8% more connections
/// opened. The worst case is three `connect_timeout`s inside one call, and it
/// takes a peer whose connects are slow and succeed, over and over: one that
/// fails leaves through the `?` below without taking another turn.
const CONNECT_ATTEMPTS: usize = 3;

/// Milliseconds since this process started.
///
/// Monotonic on purpose. An `Instant` cannot live in an atomic, and a wall clock
/// that steps backwards would retire connections that are in constant use.
fn elapsed_millis() -> u64 {
    static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
    EPOCH.elapsed().as_millis() as u64
}

/// When a connection was last handed to a caller, and how many requests are
/// riding it now.
///
/// Shared between the connection and every client holding it, so the sweep and
/// the callers cannot disagree about whether it is still in use.
#[derive(Clone)]
struct LastUsed(Arc<InUse>);

struct InUse {
    at: AtomicU64,
    riding: AtomicU64,
}

impl LastUsed {
    fn now() -> Self {
        Self(Arc::new(InUse {
            at: AtomicU64::new(elapsed_millis()),
            riding: AtomicU64::new(0),
        }))
    }

    fn touch(&self) {
        self.0.at.store(elapsed_millis(), Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        Duration::from_millis(elapsed_millis().saturating_sub(self.0.at.load(Ordering::Relaxed)))
    }

    /// Whether anyone is still using this connection: a request on it now, or a
    /// caller handed it within `idle_ttl`.
    ///
    /// A request can run for far longer than the sweep's idea of cold — agent
    /// invocations have no bound — and only ever touches the moment it is
    /// handed the connection. Judged on that alone, a connection carrying a long
    /// invocation and nothing else reads as one nobody is calling.
    /// A request that never returns keeps its connection out of the sweep for
    /// good, which is the honest reading of in use: nothing else ends such a
    /// request either, since `request_timeout` is deliberately unset.
    ///
    /// Acquire against the release in `Riding::drop`, so a sweep that sees the
    /// count reach zero also sees the timestamp the request left behind. Without
    /// the pair, aarch64 may reorder either side and the sweep judges a
    /// connection whose request has just ended by a timestamp from before it
    /// began.
    fn in_use_within(&self, idle_ttl: Duration) -> bool {
        self.0.riding.load(Ordering::Acquire) > 0 || self.idle_for() < idle_ttl
    }

    /// Counts a request as riding this connection until the guard is dropped.
    #[must_use]
    fn riding(&self) -> Riding {
        self.0.riding.fetch_add(1, Ordering::Relaxed);
        Riding(self.clone())
    }
}

/// A request riding a connection. Dropping it also touches, so the sweep starts
/// counting from when the request ended rather than from when it began.
struct Riding(LastUsed);

impl Drop for Riding {
    fn drop(&mut self) {
        self.0.touch();
        self.0.0.riding.fetch_sub(1, Ordering::Release);
    }
}

/// Whether a connection has been retired, and whether it is also gone.
///
/// Two questions, because they have different answers. Anything that retires a
/// connection stops it being handed out or installed again. Only a connection
/// whose transport failed is also gone, and being gone is what releases the
/// requests already riding it: a status the peer sent back is proof the
/// transport works, however unwelcome the answer, and cutting those requests
/// short would cost every one of them a retry.
///
/// Reading only the second question is how a connection one caller had just
/// evicted came to be installed again by a sibling partway through installing
/// it.
#[derive(Clone)]
struct Retirement {
    retired: CancellationToken,
    /// A cached `Channel` reconnects on its own, one queued request at a time,
    /// and nothing above it can see those requests to share an attempt between
    /// them. Releasing them at the moment the connection is known dead is what
    /// keeps a cohort to one `connect_timeout` between them.
    gone: CancellationToken,
}

impl Retirement {
    fn new() -> Self {
        Self {
            retired: CancellationToken::new(),
            gone: CancellationToken::new(),
        }
    }

    /// Retires the connection, and releases the requests riding it if it is
    /// gone.
    ///
    /// Callers do this before taking the lock that clears the caches, and an
    /// installer reads it while holding that lock. So either the retirement
    /// lands first and the installer refuses, or it lands afterwards and the
    /// clearing takes out what the installer put in. Reading it before taking
    /// the lock leaves an interleaving where neither happens.
    ///
    /// Retired before released, in that order, so a request woken by the
    /// release cannot read this connection as live on its way back for another.
    fn retire(&self, gone: bool) {
        self.retired.cancel();
        if gone {
            self.gone.cancel();
        }
    }

    fn is_retired(&self) -> bool {
        self.retired.is_cancelled()
    }
}

/// An established connection, the client built on it, and what tells it apart
/// from the connection that replaces it.
#[derive(Clone)]
pub struct GrpcClientConnection<T: Clone> {
    client: T,
    /// Distinguishes this connection from its successor, so a call that fails
    /// late cannot evict a replacement it never used.
    id: u64,
    retirement: Retirement,
    last_used: LastUsed,
}

fn connect_shared<T>(
    endpoint: Endpoint,
    config: &GrpcClientConfig,
    build_client: ClientFactory<T>,
) -> SharedConnect<T>
where
    T: Clone + Send + Sync + 'static,
{
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let connect_timeout = config.connect_timeout;
    let max_message_size = config.max_message_size;

    async move {
        match tokio::time::timeout(connect_timeout, endpoint.connect()).await {
            // The client is built here rather than when the connection is handed
            // over, so there is no moment in which the connection exists and the
            // client on it does not. That moment was where a sibling's
            // retirement used to slip through: it cleared the caches, and the
            // caller still partway through building put its connection back.
            Ok(Ok(channel)) => Ok(GrpcClientConnection {
                client: build_client(
                    ServiceBuilder::new().layer(OtelGrpcLayer).service(channel),
                    max_message_size,
                ),
                id,
                retirement: Retirement::new(),
                last_used: LastUsed::now(),
            }),
            Ok(Err(err)) => Err(Status::unavailable(format!("tcp connect error: {err}"))),
            Err(_) => Err(Status::unavailable(format!(
                "connection not established within {connect_timeout:?}"
            ))),
        }
    }
    .boxed()
    .shared()
}

/// The connection to each target, and any attempt still being made to reach one.
///
/// Callers wanting the same target share a single attempt, so an unreachable peer
/// costs the whole cohort one `connect_timeout` between them rather than one
/// each. A successful attempt is then kept rather than cleared, because it is
/// what every later caller reuses.
///
/// One map, holding the client rather than the channel it was built on. The
/// second map this used to keep alongside it is where a connection one caller
/// had just retired could reappear: the two were held in step by the order their
/// writes were made in, and every rule about that order was a rule somebody had
/// to go on keeping.
#[derive(Clone)]
struct Connections<T: Clone> {
    by_target: Arc<scc::HashMap<Uri, SharedConnect<T>>>,
    /// When the idle connections were last dropped, as [`elapsed_millis`].
    last_pruned: Arc<AtomicU64>,
    idle_ttl: Duration,
    prune_interval: Duration,
    /// Test-only seam: awaited between a connection being established and the
    /// retirement check below it, so a test can retire one in that window
    /// rather than race to. Same pattern as `Cache::evict_interleave` in
    /// `golem-common/src/cache.rs`.
    #[cfg(test)]
    interleave: Arc<std::sync::Mutex<Option<ConnectInterleaveHook>>>,
}

#[cfg(test)]
type ConnectInterleaveHook = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

impl<T> Connections<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn new(idle_ttl: Duration, prune_interval: Duration) -> Self {
        Self {
            by_target: Arc::new(scc::HashMap::new()),
            last_pruned: Arc::new(AtomicU64::new(elapsed_millis())),
            idle_ttl,
            prune_interval,
            #[cfg(test)]
            interleave: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[cfg(test)]
    fn set_interleave(&self, hook: ConnectInterleaveHook) {
        *self.interleave.lock().unwrap() = Some(hook);
    }

    #[cfg(test)]
    async fn interleave(&self) {
        let hook = self.interleave.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook().await;
        }
    }

    /// Returns a connected client for `target`, establishing the connection if
    /// there is not one yet and joining an attempt already under way if there
    /// is.
    async fn connect(
        &self,
        target: Uri,
        config: &GrpcClientConfig,
        build_client: &ClientFactory<T>,
    ) -> Result<GrpcClientConnection<T>, Status> {
        self.prune_if_due().await;

        let mut attempts_left = CONNECT_ATTEMPTS;
        loop {
            let connected = self.connect_once(&target, config, build_client).await?;

            #[cfg(test)]
            self.interleave().await;

            if !connected.retirement.is_retired() {
                return Ok(connected);
            }

            // A caller that joined an attempt already under way can be handed a
            // connection an earlier one has since used and given up on. Retiring
            // it also forgot it, so going back gets a fresh connection rather
            // than the same verdict.
            attempts_left = attempts_left.saturating_sub(1);
            if attempts_left == 0 {
                return Err(Status::unavailable(
                    "connection retired before it could be used",
                ));
            }
        }
    }

    /// One turn of [`Self::connect`]: the cached connection, the attempt someone
    /// else is making, or a new attempt of this caller's own.
    async fn connect_once(
        &self,
        target: &Uri,
        config: &GrpcClientConfig,
        build_client: &ClientFactory<T>,
    ) -> Result<GrpcClientConnection<T>, Status> {
        // A shared read first, before anything is built. Every caller but the one
        // that starts an attempt gets its answer here, and `build_endpoint` is
        // not free: with TLS enabled it parses the CA, client certificate and key
        // to build a fresh rustls config each time, which measures about 17.9us
        // against 94ns without. During the reconnect storm this type exists to
        // handle, that is the whole waiting cohort paying for a connection one of
        // them is already making.
        if let Some(existing) = self
            .by_target
            .read_async(target, |_, attempt| attempt.clone())
            .await
        {
            match existing.peek() {
                // Already connected.
                Some(Ok(connected)) if !connected.retirement.is_retired() => {
                    connected.last_used.touch();
                    return Ok(connected.clone());
                }
                // Someone else is connecting; wait on theirs. What it resolves
                // to may since have been retired, which the caller above checks.
                None => return existing.await,
                // A failed attempt is not worth inheriting, and neither is a
                // retired one: `retire` marks before it forgets, so between
                // those two moments this is still the cached answer and it
                // describes a connection nobody is meant to be given. Fall
                // through and start a fresh one below.
                Some(_) => {}
            }
        }

        // Built outside the entry lock, so a rejected URI cannot leave an entry
        // behind and unrelated targets sharing a bucket do not serialise behind
        // this work.
        let endpoint = build_endpoint(target.clone(), config)
            .map_err(|err| Status::from_error(Box::new(err)))?;

        let attempt = match self.by_target.entry_async(target.clone()).await {
            Entry::Occupied(mut entry) => match entry.get().peek() {
                // Already connected. This is the reuse path.
                Some(Ok(connected)) if !connected.retirement.is_retired() => {
                    connected.last_used.touch();
                    return Ok(connected.clone());
                }
                None => entry.get().clone(),
                // A failed attempt describes a connection nobody is still making
                // and a retired one describes a connection its owner has given
                // up on, so the caller gets its own rather than inheriting
                // either verdict.
                Some(_) => {
                    let attempt = connect_shared(endpoint, config, build_client.clone());
                    *entry.get_mut() = attempt.clone();
                    self.drive(target.clone(), attempt.clone());
                    attempt
                }
            },
            Entry::Vacant(entry) => {
                let attempt = connect_shared(endpoint, config, build_client.clone());
                entry.insert_entry(attempt.clone());
                self.drive(target.clone(), attempt.clone());
                attempt
            }
        };

        attempt.await
    }

    /// Drives an attempt to completion independently of its callers, and clears
    /// it if it failed. Called at every point an attempt is created, with
    /// nothing awaited in between, so that no arm can create one and leave it
    /// with nobody to finish it however its caller goes away.
    ///
    /// Callers go away all the time: an upstream timeout, a dropped client. An
    /// attempt driven only by its waiters would, on losing the last of them, sit
    /// parked mid-connect with its deadline quietly expiring, and the next caller
    /// would inherit an instant verdict about a connection nobody ever made.
    ///
    /// A successful attempt is deliberately left in place. Clearing it opens a
    /// window, however brief, in which the connection is nowhere to be found: a
    /// caller looking just after it was cleared starts a second one. Under load
    /// that window is wide enough to hit.
    fn drive(&self, target: Uri, attempt: SharedConnect<T>) -> JoinHandle<()> {
        let by_target = self.by_target.clone();
        tokio::spawn(async move {
            if attempt.clone().await.is_err() {
                by_target
                    .remove_if_async(&target, |current| current.ptr_eq(&attempt))
                    .await;
            }
        })
    }

    /// Drops the connections to targets nobody has called lately, at most once
    /// every `prune_interval` however many callers arrive.
    ///
    /// Run by every caller rather than only by one that missed the cache. A miss
    /// means a target nobody has reached before, and a cluster whose targets are
    /// all connected stops producing them, so hanging this off a miss meant it
    /// stopped running in exactly the steady state it was written for.
    ///
    /// Whoever moves the timestamp does the work and the rest carry on. A caller
    /// that loses the exchange has nothing to wait for, since the sweep it lost
    /// to covers the same connections its own would have.
    async fn prune_if_due(&self) {
        let now = elapsed_millis();
        let last = self.last_pruned.load(Ordering::Relaxed);
        if Duration::from_millis(now.saturating_sub(last)) < self.prune_interval {
            return;
        }
        if self
            .last_pruned
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        self.prune(self.idle_ttl).await;
    }

    /// Drops connections to targets nobody has called for `idle_ttl`.
    ///
    /// An attempt still in flight is never dropped: it has no outcome to have
    /// been idle with, and callers are waiting on it right now.
    async fn prune(&self, idle_ttl: Duration) {
        self.by_target
            .retain_async(|_, attempt| match attempt.peek() {
                Some(Ok(connected)) => connected.last_used.in_use_within(idle_ttl),
                _ => true,
            })
            .await;
    }

    /// Forgets connection `id` to `target`, so the next call builds a new one.
    /// Used when a connection turns out to be dead.
    ///
    /// Matching on the identity rather than the target alone covers two things
    /// at once. An attempt still in flight never matches, so it is left to the
    /// callers waiting on it rather than being taken from them. And a connection
    /// that has already been replaced never matches either, so a call that fails
    /// late cannot discard the replacement its own retry established.
    async fn forget(&self, target: &Uri, id: u64) {
        self.by_target
            .remove_if_async(
                target,
                |attempt| matches!(attempt.peek(), Some(Ok(connected)) if connected.id == id),
            )
            .await;
    }
}

/// A client for one fixed target.
///
/// A [`MultiTargetGrpcClient`] that never names more than one target. The two
/// were written out separately once, and drifted apart as such pairs do: the
/// single-target call loop ended up with no test reaching it at all, and the
/// stall this file exists to fix lived in that half.
#[derive(Clone)]
pub struct GrpcClient<T: Clone> {
    endpoint: Uri,
    inner: MultiTargetGrpcClient<T>,
}

impl<T> GrpcClient<T>
where
    T: Clone + Send + Sync + 'static,
{
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static,
        endpoint: Uri,
        config: GrpcClientConfig,
    ) -> Self {
        Self {
            endpoint,
            // Never dropped for being idle. See [`IDLE_CONNECTION_TTL`].
            inner: MultiTargetGrpcClient::with_sweep(
                target_name,
                client_factory,
                config,
                Duration::MAX,
                PRUNE_INTERVAL,
            ),
        }
    }

    pub async fn call<F, R>(&self, description: impl AsRef<str>, f: F) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        self.inner.call(description, self.endpoint.clone(), f).await
    }
}

#[derive(Clone)]
pub struct MultiTargetGrpcClient<T: Clone> {
    config: GrpcClientConfig,
    connections: Connections<T>,
    client_factory: ClientFactory<T>,
    target_name: String,
}

impl<T> MultiTargetGrpcClient<T>
where
    T: Clone + Send + Sync + 'static,
{
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static,
        config: GrpcClientConfig,
    ) -> Self {
        Self::with_sweep(
            target_name,
            client_factory,
            config,
            IDLE_CONNECTION_TTL,
            PRUNE_INTERVAL,
        )
    }

    fn with_sweep(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static,
        config: GrpcClientConfig,
        idle_ttl: Duration,
        prune_interval: Duration,
    ) -> Self {
        Self {
            config,
            connections: Connections::new(idle_ttl, prune_interval),
            client_factory: Arc::new(client_factory),
            target_name: target_name.as_ref().to_string(),
        }
    }

    pub async fn call<F, R>(
        &self,
        description: impl AsRef<str>,
        endpoint: Uri,
        f: F,
    ) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        let description = description.as_ref();
        let mut attempts = CallAttempts::new(
            &self.config.retries_on_unavailable,
            &self.target_name,
            description,
            debug_span!(
                "gRPC call",
                target_name = self.target_name,
                endpoint = endpoint.to_string(),
                description = description
            ),
        );
        // Timed from before the first attempt, because that is the whole of what
        // the caller waits for: a call that failed to connect, backed off and
        // succeeded on its third attempt cost them all three. Establishment used
        // to happen inside the RPC, so timing from any point below here drops a
        // whole `connect_timeout` out of `internal_grpc_success_seconds` on
        // exactly the reconnecting calls this is about.
        let started = Instant::now();
        loop {
            attempts.start();
            // A failed connection attempt goes through the same retry path as a
            // failed call, so `retries_on_unavailable` still governs it.
            let mut entry = match self
                .connections
                .connect(endpoint.clone(), &self.config, &self.client_factory)
                .await
            {
                Ok(entry) => entry,
                Err(e) => match attempts.failed("gRPC connect", &e).await {
                    NextAttempt::Retry => continue,
                    NextAttempt::GiveUp => break Err(e),
                },
            };

            match attempt(&mut entry, &f, &attempts).await {
                Ok(result) => {
                    attempts.succeeded(started.elapsed());
                    break Ok(result);
                }
                Err(e) if requires_reconnect(&e) => {
                    self.retire(&endpoint, &entry, &e).await;
                    match attempts.failed("gRPC call", &e).await {
                        NextAttempt::Retry => continue,
                        NextAttempt::GiveUp => break Err(e),
                    }
                }
                Err(e) => {
                    attempts.gave_up(&e);
                    break Err(e);
                }
            }
        }
    }

    pub fn uses_tls(&self) -> bool {
        self.config.tls_enabled()
    }

    /// Retires the connection a failed call was riding: marks it so nothing
    /// hands it out again, and forgets it so the next call builds a new one.
    ///
    /// Keyed on the connection's identity rather than on the target alone. Two
    /// callers can fail on the same connection at different moments, and without
    /// that the later failure would evict the replacement the earlier one had
    /// already established.
    ///
    /// Only a failure meaning the connection is gone also releases the requests
    /// still riding it; see [`Retirement`].
    async fn retire(&self, endpoint: &Uri, connection: &GrpcClientConnection<T>, cause: &Status) {
        connection.retirement.retire(connection_is_gone(cause));
        self.connections.forget(endpoint, connection.id).await;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcClientConfig {
    /// Bounds connection establishment to one target.
    ///
    /// Callers waiting for the same target share a single attempt, so an
    /// unreachable peer costs the whole waiting cohort one `connect_timeout`
    /// between them rather than one each. Before that was so, requests queued
    /// inside tonic's `Channel` and each waited for those ahead of it to time
    /// out in turn — which produced the 119,781ms stalls in chaos run S5 on
    /// 2026-08-19, roughly twelve requests deep against a 10s timeout.
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
    #[serde(default, with = "humantime_serde::option")]
    pub request_timeout: Option<Duration>,
    /// How often to send an HTTP/2 PING on an established connection. Without
    /// this, a peer that disappears while a connection is open is only noticed
    /// when the kernel gives up retransmitting, which takes minutes.
    ///
    /// The default suits golem talking to golem, where both ends are `h2`, which
    /// enforces no minimum ping interval. Servers that do enforce one answer a
    /// faster idle ping with GOAWAY: grpc-core defaults to 300s and grpc-go's
    /// server to 5 minutes. Pointing one of these clients at a non-Rust gRPC
    /// server, or through a proxy that enforces the policy, wants this raised.
    #[serde(default, with = "humantime_serde::option")]
    pub http2_keep_alive_interval: Option<Duration>,
    /// How long to wait for the PING ack before considering the connection dead.
    #[serde(default, with = "humantime_serde::option")]
    pub http2_keep_alive_timeout: Option<Duration>,
    /// Whether to keep pinging when no requests are in flight.
    #[serde(default)]
    pub http2_keep_alive_while_idle: Option<bool>,
    pub retries_on_unavailable: RetryConfig,
    pub tls: GrpcClientTlsConfig,
    #[serde(default = "default_max_message_size")]
    pub max_message_size: usize,
}

fn default_max_message_size() -> usize {
    32 * 1024 * 1024
}

impl GrpcClientConfig {
    pub fn tls_enabled(&self) -> bool {
        matches!(self.tls, GrpcClientTlsConfig::Enabled(_))
    }
}

impl Default for GrpcClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: None,
            // `connect_timeout` only bounds the TCP connect, and `request_timeout`
            // is deliberately unset because agent invocations run arbitrarily
            // long. Without keep-alive, a peer that disappears while a connection
            // is open is therefore unbounded: the call waits until the kernel
            // gives up retransmitting. Pinging bounds detection at roughly
            // interval + timeout without capping legitimate request duration.
            http2_keep_alive_interval: Some(Duration::from_secs(10)),
            http2_keep_alive_timeout: Some(Duration::from_secs(10)),
            // Cached channels sit idle between requests; ping then too, so a dead
            // peer is discovered before the next request rather than during it.
            http2_keep_alive_while_idle: Some(true),
            retries_on_unavailable: RetryConfig::default(),
            tls: GrpcClientTlsConfig::Disabled(Empty {}),
            max_message_size: default_max_message_size(),
        }
    }
}

impl SafeDisplay for GrpcClientConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "connect_timeout: {:?}", self.connect_timeout);
        let _ = writeln!(&mut result, "request_timeout: {:?}", self.request_timeout);
        let _ = writeln!(
            &mut result,
            "http2_keep_alive_interval: {:?}",
            self.http2_keep_alive_interval
        );
        let _ = writeln!(
            &mut result,
            "http2_keep_alive_timeout: {:?}",
            self.http2_keep_alive_timeout
        );
        let _ = writeln!(
            &mut result,
            "http2_keep_alive_while_idle: {:?}",
            self.http2_keep_alive_while_idle
        );
        let _ = writeln!(&mut result, "max_message_size: {}", self.max_message_size);
        let _ = writeln!(&mut result, "retries_on_unavailable:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.retries_on_unavailable.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum GrpcClientTlsConfig {
    Enabled(EnabledGrpcClientTlsConfig),
    Disabled(Empty),
}

impl SafeDisplay for GrpcClientTlsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            Self::Enabled(inner) => {
                let _ = writeln!(&mut result, "Enabled:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            Self::Disabled(_) => {
                let _ = writeln!(&mut result, "Disabled");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnabledGrpcClientTlsConfig {
    /// client-specific certificate  — issued by cluster CA
    pub client_cert: Base64,
    /// private key for client_cert
    pub client_key: Base64,
    /// CA certificate used to validate server certificates (PEM)
    pub server_ca_cert: Base64,
    /// expected server domain/SAN. If None the domain name validation is disabled
    pub server_domain_name: Option<String>,
}

impl SafeDisplay for EnabledGrpcClientTlsConfig {
    fn to_safe_string(&self) -> String {
        use sha2::{Digest, Sha256};

        fn fingerprint(data: &[u8]) -> String {
            let hash = Sha256::digest(data);
            hex::encode(hash)
        }

        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "client_cert_sha256: {}",
            fingerprint(&self.client_cert.0)
        );
        let _ = writeln!(&mut result, "client_key: *******");
        let _ = writeln!(
            &mut result,
            "server_ca_cert_sha256: {}",
            fingerprint(&self.server_ca_cert.0)
        );

        let _ = writeln!(
            &mut result,
            "server_domain_name: {:?}",
            self.server_domain_name
        );
        result
    }
}

impl EnabledGrpcClientTlsConfig {
    pub fn to_tonic(&self) -> ClientTlsConfig {
        use tonic::transport::{Certificate, Identity};

        let ca = Certificate::from_pem(&self.server_ca_cert.0);
        let identity = Identity::from_pem(&self.client_cert.0, &self.client_key.0);

        let mut config = ClientTlsConfig::new().ca_certificate(ca).identity(identity);

        if let Some(domain_name) = &self.server_domain_name {
            config = config.domain_name(domain_name);
        }

        config
    }
}

/// Runs one attempt, and gives up the moment the connection it is riding is
/// gone.
///
/// A request queued inside a dead `Channel` is waiting its turn to re-dial, and
/// every turn costs a whole `connect_timeout`. tonic does that re-dialling below
/// anything we can reach, one queued request at a time, so sharing a connection
/// attempt above it never sees them. Releasing the queue as soon as any one of
/// them proves the connection dead is what keeps the cohort to one
/// `connect_timeout` between them rather than one each.
async fn attempt<T, F, R>(
    connection: &mut GrpcClientConnection<T>,
    f: &F,
    attempts: &CallAttempts<'_>,
) -> Result<R, Status>
where
    T: Clone,
    F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>> + Send,
{
    let _riding = connection.last_used.riding();
    tokio::select! {
        // A result that is already there wins over a retirement landing in the
        // same tick.
        biased;
        result = f(&mut connection.client).instrument(attempts.span()) => result,
        () = connection.retirement.gone.cancelled() => Err(Status::unavailable(
            "connection retired while the request was waiting on it",
        )),
    }
}

/// Whether a `call` loop should go round again.
enum NextAttempt {
    Retry,
    GiveUp,
}

/// The retry bookkeeping both clients' `call` loops share: decide whether there
/// is another attempt to make, and record the outcome either way.
///
/// Kept in one place because the two loops previously carried byte-identical
/// copies of it, which is how more than one fix in this area came to be needed
/// twice.
struct CallAttempts<'a> {
    retries: RetryState<'a>,
    target_name: &'a str,
    description: &'a str,
    span: tracing::Span,
}

impl<'a> CallAttempts<'a> {
    fn new(
        config: &'a RetryConfig,
        target_name: &'a str,
        description: &'a str,
        span: tracing::Span,
    ) -> Self {
        Self {
            retries: RetryState::new(config),
            target_name,
            description,
            span,
        }
    }

    fn span(&self) -> tracing::Span {
        self.span.clone()
    }

    fn start(&mut self) {
        self.retries.start_attempt();
    }

    /// Records a failed attempt and says whether to try again. `what` names the
    /// step that failed, so a connect failure reads differently from a call that
    /// reached the peer.
    async fn failed(&mut self, what: &str, e: &Status) -> NextAttempt {
        if self.retries.failed_attempt().await {
            self.span
                .in_scope(|| debug!("{what} failed with {:?}, retrying", e));
            record_internal_grpc_retry(self.target_name, self.description);
            NextAttempt::Retry
        } else {
            self.span
                .in_scope(|| warn!("{what} failed: {:?}, no more retries", e));
            record_internal_grpc_failure(self.target_name, self.description, e);
            NextAttempt::GiveUp
        }
    }

    /// Records a failure there is no point retrying.
    fn gave_up(&self, e: &Status) {
        self.span
            .in_scope(|| warn!("gRPC call failed: {:?}, not retriable", e));
        record_internal_grpc_failure(self.target_name, self.description, e);
    }

    fn succeeded(&self, took: Duration) {
        record_internal_grpc_success(self.target_name, self.description, took);
    }
}

fn requires_reconnect(e: &Status) -> bool {
    // A request that ran out of its own time says nothing about the health of
    // the connection it ran on. Without this the registry client
    // (request_timeout = 30s) would tear down the channel every caller shares
    // each time one request ran long.
    if request_timed_out(e) {
        return false;
    }

    worth_reconnecting(e.code(), has_transport_source(e))
}

/// Whether the cached connection is worth replacing, given what a status says
/// and whether the transport under it failed.
///
/// Wider than `connection_is_gone` by one case and only one: `Unavailable`, from
/// wherever it came. A peer that answers `Unavailable` itself is usually still
/// reachable, so this is imprecise and known to be, at the cost of a connection
/// that did not need replacing. Narrowing it is a change of its own.
///
/// Everything else has to say the transport failed. A dead connection reaches us
/// as `Unknown` or `Cancelled` with a `tonic::transport::Error` under it, and
/// matching only on `Unavailable` left such channels cached indefinitely, so
/// every later request queued onto a connection that could never work again.
/// Reading the source alone, as this once did, took every other code with it,
/// including the ones tonic derives from an HTTP/2 reset: each of those ends one
/// stream and leaves the connection carrying everybody else.
fn worth_reconnecting(code: Code, transport_failed: bool) -> bool {
    code == Code::Unavailable || (code_means_connection_gone(code) && transport_failed)
}

/// Whether the request ran out of its own time, rather than the connection
/// having failed under it.
///
/// tonic reports both as `Cancelled` with a transport error attached, so the
/// source chain is the only thing separating them. A `request_timeout` carries a
/// `tonic::TimeoutExpired`; a connection that closed with the request still on it
/// carries a hyper error reporting itself cancelled.
///
/// Excluding `Cancelled` wholesale, as this once did, threw away the commonest
/// evidence a connection has died: killing a pod with requests in flight reports
/// most of them that way.
fn request_timed_out(e: &Status) -> bool {
    let mut source = std::error::Error::source(e);
    while let Some(err) = source {
        if err.is::<tonic::TimeoutExpired>() {
            return true;
        }
        source = err.source();
    }
    false
}

/// Whether the connection carrying this request is gone, as opposed to one
/// request on it having gone wrong.
///
/// This is a stricter question than [`requires_reconnect`], and deliberately so.
/// Dropping a connection from the cache costs one reconnect, so it is worth doing
/// on a hint. Declaring it gone releases every other request riding it, so it
/// needs to be right.
///
/// The source alone cannot answer it. `Channel` reports every failure as a
/// `tonic::transport::Error`, a single reset stream included, so the code is what
/// separates them. Measured against a real peer: a connect into a blackhole and
/// an expired keep-alive ping arrive as `Unavailable`, a connection the kernel
/// gives up on arrives as `Unknown`, and a connection that closes with requests
/// still on it arrives as `Cancelled` — the commonest of the three, because it is
/// what killing a busy pod looks like.
///
/// `Internal`, `ResourceExhausted` and `PermissionDenied` are left out. tonic
/// derives those from an HTTP/2 reset, which ends one stream and leaves the
/// connection carrying everyone else.
///
/// The split is not clean, and cannot be. `REFUSED_STREAM` also resets a single
/// stream and also arrives as `Unavailable`, where an expired keep-alive ping
/// arrives too, so no code tells those two apart. Reading a refused stream as a
/// dead connection costs a reconnect and a retry, which is the cheaper way to be
/// wrong.
fn connection_is_gone(e: &Status) -> bool {
    if request_timed_out(e) {
        return false;
    }
    code_means_connection_gone(e.code()) && has_transport_source(e)
}

fn code_means_connection_gone(code: Code) -> bool {
    matches!(code, Code::Unavailable | Code::Unknown | Code::Cancelled)
}

fn has_transport_source(e: &Status) -> bool {
    std::error::Error::source(e)
        .map(|source| source.is::<tonic::transport::Error>())
        .unwrap_or(false)
}

#[cfg(test)]
mod test {
    use super::*;
    use test_r::test;

    /// A peer that completes the TCP handshake and then holds the socket open.
    ///
    /// `Endpoint::connect` performs the TCP connect and sends its own HTTP/2
    /// preface without waiting for a reply, so this is enough for a connection to
    /// establish. Nothing here sends a request over one.
    async fn silent_peer() -> (Uri, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let held = tokio::spawn(async move {
            let mut sockets = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                sockets.push(socket);
            }
        });
        (format!("http://{addr}").parse().unwrap(), held)
    }

    /// A bare cache and the factory that goes with it, for tests that drive
    /// [`Connections`] rather than a client. Swept only when a test asks: the
    /// interval keeps [`Connections::prune_if_due`] out of the way of tests that
    /// are about something else.
    fn cache() -> (Connections<()>, ClientFactory<()>) {
        (
            Connections::new(IDLE_CONNECTION_TTL, PRUNE_INTERVAL),
            Arc::new(|_, _| ()),
        )
    }

    /// An attempt a test can put into the cache itself, with a connect timeout
    /// of its own.
    fn attempt_bounded_by(endpoint: Endpoint, connect_timeout: Duration) -> SharedConnect<()> {
        connect_shared(
            endpoint,
            &GrpcClientConfig {
                connect_timeout,
                ..GrpcClientConfig::default()
            },
            Arc::new(|_, _| ()),
        )
    }

    /// The connection a caller would be given for `target`, which is what
    /// [`MultiTargetGrpcClient::call`] asks for on every attempt.
    async fn connect(
        client: &MultiTargetGrpcClient<()>,
        target: Uri,
    ) -> Result<GrpcClientConnection<()>, Status> {
        client
            .connections
            .connect(target, &client.config, &client.client_factory)
            .await
    }

    /// Two callers can fail on the same connection at different moments. The
    /// later failure must not discard the replacement the earlier one's retry has
    /// already established, which is what keying eviction on the target alone
    /// used to do: the third caller then opened a third connection.
    #[test]
    async fn forget_only_removes_the_connection_it_names() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let (connections, build) = cache();

        let first = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();

        // A failure arriving late, naming a connection that has already been
        // replaced.
        connections.forget(&target, first.id + 1).await;
        let reused = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_eq!(
            reused.id, first.id,
            "forget discarded a connection whose identity it was not given"
        );

        // The connection that actually failed does go.
        connections.forget(&target, first.id).await;
        let replacement = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_ne!(
            replacement.id, first.id,
            "forget left in place the very connection it was given"
        );
    }

    /// A config that makes one attempt and gives up, so a test driving `call`
    /// measures the one failure it set up rather than a retry loop.
    fn one_attempt() -> GrpcClientConfig {
        GrpcClientConfig {
            retries_on_unavailable: RetryConfig {
                max_attempts: 0,
                ..RetryConfig::default()
            },
            ..GrpcClientConfig::default()
        }
    }

    /// Counts how many times the client factory ran, which is once per
    /// connection actually installed.
    fn counting_client() -> (MultiTargetGrpcClient<()>, Arc<AtomicU64>) {
        let built = Arc::new(AtomicU64::new(0));
        let counter = built.clone();
        let client = MultiTargetGrpcClient::new(
            "test",
            move |_, _| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
            GrpcClientConfig::default(),
        );
        (client, built)
    }

    /// A `Status` that `connection_is_gone` accepts, built from a connect that
    /// really failed because `tonic::transport::Error` has no way to construct
    /// one directly.
    async fn dead_transport_status() -> Status {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let endpoint = build_endpoint(
            format!("http://{addr}").parse().unwrap(),
            &GrpcClientConfig::default(),
        )
        .unwrap();
        Status::from_error(Box::new(endpoint.connect().await.unwrap_err()))
    }

    /// Retires a connection the way a status the peer sent back does: marked,
    /// and nothing released. The harder half of every guard below, because a
    /// guard reading only whether the connection is gone reads nothing at all
    /// here — and it is what a retirement whose caller was dropped partway
    /// through leaves behind as well.
    fn retired_by_the_peer(retirement: &Retirement) {
        retirement.retire(false);
    }

    /// Bounded, because a call that never arrives would otherwise hang the whole
    /// test run rather than fail this test.
    async fn expect(what: &str, f: impl Future<Output = ()>) {
        tokio::time::timeout(Duration::from_secs(10), f)
            .await
            .unwrap_or_else(|_| panic!("{what}"));
    }

    /// The other half of [`forget_only_removes_the_connection_it_names`]. Two
    /// callers can fail on the same connection at different moments, and the
    /// later failure must leave the replacement the earlier one's retry
    /// established alone.
    ///
    /// Counted by connections built rather than by identity, because identity
    /// alone cannot see this: `Connections` still holds the replacement, so an
    /// over-eager eviction here is served the same connection back and only shows
    /// up as the wasted rebuild.
    #[test]
    async fn a_late_failure_does_not_evict_the_replacement_client() {
        let (target, _peer) = silent_peer().await;
        let (client, built) = counting_client();
        let cause = Status::unavailable("peer went away");

        let first = connect(&client, target.clone()).await.unwrap();
        client.retire(&target, &first, &cause).await;

        let replacement = connect(&client, target.clone()).await.unwrap();
        assert_ne!(
            replacement.id, first.id,
            "the first failure evicted nothing"
        );

        // A second caller, still holding the original, fails late.
        client.retire(&target, &first, &cause).await;
        let handed_out = connect(&client, target.clone()).await.unwrap();

        assert_eq!(
            handed_out.id, replacement.id,
            "a failure on the old connection sent the next caller elsewhere"
        );
        assert_eq!(
            built.load(Ordering::SeqCst),
            2,
            "a failure on the old connection threw away the replacement, so it \
             had to be built again"
        );
    }

    /// `retire` marks a connection before it forgets it, so between those two
    /// moments the cached attempt still answers with a connection nobody is
    /// meant to be given. Handing it back turns every caller arriving in that
    /// window into a failure that had no reason to happen — and a retirement
    /// that got no further than the mark, its caller dropped partway through,
    /// leaves the attempt answering that way for good.
    #[test]
    async fn a_retired_connection_is_not_handed_to_the_next_caller() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let (connections, build) = cache();

        let first = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        retired_by_the_peer(&first.retirement);

        let next = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_ne!(
            next.id, first.id,
            "a connection that had been retired was handed straight back"
        );
        assert!(
            !next.retirement.is_retired(),
            "the replacement came out already retired"
        );
    }

    /// A target that stops being called must not hold its connection for the life
    /// of the process. Executor pods come and go, and each one left behind keeps a
    /// `Channel` and the `Buffer` worker task tower spawned for it.
    ///
    /// Driven by an explicit TTL rather than by waiting, so it costs nothing and
    /// cannot flake.
    #[test]
    async fn a_target_nobody_calls_loses_its_connection() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let (connections, build) = cache();

        let first = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();

        // Just used, so no sweep should touch it.
        connections.prune(Duration::from_secs(60)).await;
        let kept = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_eq!(
            kept.id, first.id,
            "the sweep dropped a connection that was still in use"
        );

        // Against a zero TTL everything counts as cold.
        connections.prune(Duration::ZERO).await;
        let replacement = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_ne!(
            replacement.id, first.id,
            "the sweep kept a connection nobody had called"
        );
    }

    /// A request can run for far longer than the sweep's idea of cold, and only
    /// touches the moment it is handed the connection. Judged on that alone a
    /// connection carrying a long invocation and nothing else reads as one
    /// nobody is calling, and the next caller pays a reconnect for it.
    #[test]
    async fn a_connection_carrying_a_request_is_not_swept() {
        fn never(_: &mut ()) -> Pin<Box<dyn Future<Output = Result<(), Status>> + Send + '_>> {
            Box::pin(std::future::pending())
        }

        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_client();
        let mut connection = connect(&client, target.clone()).await.unwrap();

        let config = RetryConfig::default();
        let attempts = CallAttempts::new(&config, "test", "test", tracing::Span::none());
        // Boxed rather than `pin!`ed because this test has to drop the request
        // itself, and dropping a `Pin<&mut _>` drops the borrow rather than what
        // it points at.
        let mut running = Box::pin(attempt(&mut connection, &never, &attempts));
        assert!(futures::poll!(running.as_mut()).is_pending());

        // Against a zero TTL everything that is merely idle counts as cold.
        client.connections.prune(Duration::ZERO).await;
        assert!(
            client.connections.by_target.contains_async(&target).await,
            "the sweep took a connection with a request still riding it"
        );

        drop(running);
        client.connections.prune(Duration::ZERO).await;
        assert!(
            !client.connections.by_target.contains_async(&target).await,
            "the connection stayed exempt from the sweep after its request \
             had ended"
        );
    }

    /// A caller cannot go back for another connection for ever. The seam retires
    /// every connection the caller establishes, so it has to run out of attempts
    /// and say so rather than spin against a target something else keeps taking
    /// out from under it.
    ///
    /// Counted as well as bounded, because the count is what says where it
    /// stopped. Each turn costs a connection built and thrown away, which is
    /// what building the client with the connection rather than after it buys
    /// the window it closes.
    #[test]
    async fn a_caller_stops_going_back_for_another_connection() {
        let (target, _peer) = silent_peer().await;
        let (client, built) = counting_client();

        // The connection already cached is retired too, so the caller has to get
        // past that one as well, which is how it comes to be reading an entry
        // rather than an empty cache.
        let cached = connect(&client, target.clone()).await.unwrap();
        retired_by_the_peer(&cached.retirement);
        let built_before = built.load(Ordering::SeqCst);

        // Something is retiring connections to this target as fast as they are
        // made: whatever the caller has just been handed goes the moment it has
        // it, so every turn round the loop finds the same verdict.
        let by_target = client.connections.by_target.clone();
        let retire_target = target.clone();
        client.connections.set_interleave(Arc::new(move || {
            let (by_target, target) = (by_target.clone(), retire_target.clone());
            async move {
                if let Some(attempt) = by_target.read_async(&target, |_, a| a.clone()).await
                    && let Some(Ok(connected)) = attempt.peek()
                {
                    retired_by_the_peer(&connected.retirement);
                }
            }
            .boxed()
        }));

        let mut connected = None;
        expect(
            "the caller never stopped going back for another connection",
            async {
                connected = Some(connect(&client, target.clone()).await);
            },
        )
        .await;

        assert!(
            connected.unwrap().is_err(),
            "a caller was handed a connection every one of whose predecessors \
             had been retired"
        );
        assert_eq!(
            built.load(Ordering::SeqCst) - built_before,
            CONNECT_ATTEMPTS as u64,
            "the caller did not go back exactly {CONNECT_ATTEMPTS} times before \
             giving up"
        );
    }

    /// The single-target client is the multi-target one with its target filled
    /// in, so what needs checking is that the target reaches it and that its one
    /// connection is exempt from the sweep.
    ///
    /// One test rather than a copy of each above, because there is now one
    /// implementation rather than two. The two drifted apart while they were
    /// separate: the single-target call loop ended up with no test reaching it
    /// at all, and the stall this file exists to fix lived in that half.
    #[test]
    async fn the_single_target_client_calls_the_target_it_was_given() {
        // Refuses at once, so the failure is the connect to this target and
        // nothing slower.
        let target: Uri = "http://127.0.0.1:1/".parse().unwrap();
        let client = GrpcClient::new("test", |_, _| (), target, one_attempt());

        let failed = client
            .call("test", |_: &mut ()| Box::pin(async { Ok(()) }))
            .await
            .expect_err("nothing is listening on port 1");
        assert_eq!(failed.code(), Code::Unavailable);
        assert!(
            failed.message().contains("tcp connect error"),
            "expected the failure to come from connecting to the fixed target, \
             got: {}",
            failed.message()
        );
        assert_eq!(
            client.inner.connections.idle_ttl,
            Duration::MAX,
            "a client with one fixed target would drop its connection for \
             going idle"
        );
    }

    /// The sweep has to run on calls that find their target already connected,
    /// because in a cluster whose targets are all connected those are all the
    /// calls there are. Hanging it off a cache miss meant a target that went
    /// quiet was never dropped: nothing missed, so nothing swept, so the ten
    /// minutes this documents were never up for anybody.
    #[test]
    async fn a_call_to_a_connected_target_still_sweeps() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let build: ClientFactory<()> = Arc::new(|_, _| ());

        // Nothing is ever due, so nothing is ever swept.
        let mut connections = Connections::<()>::new(Duration::ZERO, Duration::MAX);
        let first = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        let reused = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_eq!(
            reused.id, first.id,
            "a sweep ran before its interval was up"
        );

        // Now everything is due, and every connection counts as cold. A call
        // that would otherwise have been served the cached connection sweeps it
        // first and builds another.
        connections.prune_interval = Duration::ZERO;
        let after = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_ne!(
            after.id, first.id,
            "a call to an already-connected target did not sweep, so a target \
             nobody calls keeps its connection for as long as the process lives"
        );
    }

    /// An attempt still being made belongs to the callers waiting on it, and has
    /// no last-used moment to be judged by. Sweeping it would send the next caller
    /// off to open a second connection to the same place.
    #[test]
    async fn the_sweep_leaves_an_attempt_in_flight_alone() {
        let config = GrpcClientConfig::default();
        let (connections, _build) = cache();

        // Nothing listens here, and the connect is held open by a timeout far
        // longer than the test, so the attempt stays unsettled throughout.
        let target: Uri = "http://127.0.0.1:1/".parse().unwrap();
        let endpoint = build_endpoint(target.clone(), &config).unwrap();
        let attempt = attempt_bounded_by(endpoint, Duration::from_secs(600));
        connections
            .by_target
            .insert_async(target.clone(), attempt)
            .await
            .unwrap();

        connections.prune(Duration::ZERO).await;

        assert!(
            connections.by_target.contains_async(&target).await,
            "the sweep took an attempt that callers were still waiting on"
        );
    }

    /// A status the peer sent back is proof the transport works, so it must not
    /// take down the other requests sharing that connection. An executor raising
    /// `Unavailable` while its shards move is the case that matters: retiring the
    /// connection under it would cut short every invocation riding it.
    #[test]
    async fn a_status_from_the_peer_does_not_cut_short_the_requests_on_it() {
        let from_peer = Status::unavailable("shard is being reassigned");
        assert!(
            requires_reconnect(&from_peer),
            "an Unavailable still drops the cached connection"
        );
        assert!(
            !connection_is_gone(&from_peer),
            "an Unavailable the peer sent must not cut short the requests \
             sharing its connection"
        );
    }

    /// `Channel` reports a reset stream and a dead connection alike as a
    /// transport error, so the code is the only thing separating them. Pinned
    /// against the codes tonic derives from an HTTP/2 reset, because reading one
    /// of those as a dead connection would end every request riding a connection
    /// that is still perfectly good.
    #[test]
    async fn only_some_codes_describe_a_connection_that_is_gone() {
        // Measured against a real peer: a blackholed connect and an expired
        // keep-alive ping arrive as Unavailable, a connection the kernel gives up
        // on arrives as Unknown, and a connection closing with requests still on
        // it arrives as Cancelled.
        for code in [Code::Unavailable, Code::Unknown, Code::Cancelled] {
            assert!(
                code_means_connection_gone(code),
                "{code:?} is what a dead connection arrives as"
            );
        }

        // What tonic maps the other HTTP/2 reset reasons to. Each ends one
        // stream and leaves the connection carrying everyone else.
        for code in [
            Code::Internal,
            Code::ResourceExhausted,
            Code::PermissionDenied,
        ] {
            assert!(
                !code_means_connection_gone(code),
                "{code:?} describes one reset stream, not the connection \
                 carrying it"
            );
        }
    }

    /// Replacing the connection has to be at least as willing as giving up on
    /// it, or one this client had already written off would stay cached and go
    /// on collecting requests. It is willing about one thing more, `Unavailable`
    /// from anywhere, and about nothing beyond that.
    ///
    /// Reading the source alone, as this once did, went well past that: it
    /// reconnected on any code at all as long as a transport error was attached,
    /// which takes in every code tonic derives from an HTTP/2 reset.
    #[test]
    async fn only_unavailable_reconnects_without_the_transport_having_failed() {
        for code in [
            Code::Internal,
            Code::ResourceExhausted,
            Code::PermissionDenied,
        ] {
            assert!(
                !worth_reconnecting(code, true),
                "{code:?} ends one stream, so the connection carrying it stays"
            );
        }

        for code in [Code::Unknown, Code::Cancelled] {
            assert!(
                worth_reconnecting(code, true),
                "{code:?} over a failed transport is a connection that is gone"
            );
            assert!(
                !worth_reconnecting(code, false),
                "{code:?} on its own says nothing about the transport"
            );
        }

        assert!(
            worth_reconnecting(Code::Unavailable, false),
            "Unavailable reconnects whoever sent it"
        );

        for code in ALL_CODES {
            for transport_failed in [true, false] {
                assert!(
                    !(code_means_connection_gone(code) && transport_failed)
                        || worth_reconnecting(code, transport_failed),
                    "{code:?} is gone but would not be replaced"
                );
            }
        }
    }

    /// Every code tonic can hand back, so the sweep over them below cannot go
    /// stale as one predicate or the other changes.
    const ALL_CODES: [Code; 17] = [
        Code::Ok,
        Code::Cancelled,
        Code::Unknown,
        Code::InvalidArgument,
        Code::DeadlineExceeded,
        Code::NotFound,
        Code::AlreadyExists,
        Code::PermissionDenied,
        Code::ResourceExhausted,
        Code::FailedPrecondition,
        Code::Aborted,
        Code::OutOfRange,
        Code::Unimplemented,
        Code::Internal,
        Code::Unavailable,
        Code::DataLoss,
        Code::Unauthenticated,
    ];

    /// `Cancelled` covers both a connection that closed under the request and a
    /// request that ran out of its own time, and only the source chain separates
    /// them. Reading a request timeout as a dead connection would tear down the
    /// channel every caller shares — the registry client sets a 30s
    /// `request_timeout` — and cut short everything riding it.
    #[test]
    async fn a_request_that_ran_out_of_time_is_not_a_dead_connection() {
        let timed_out = Status::from_error(Box::new(tonic::TimeoutExpired(())));
        assert_eq!(timed_out.code(), Code::Cancelled);

        assert!(request_timed_out(&timed_out));
        assert!(!requires_reconnect(&timed_out));
        assert!(!connection_is_gone(&timed_out));
    }

    /// Retires `connection` while the very lock `retire` has to take to forget
    /// it is held, and comes back once the retirement has landed.
    ///
    /// A caller reading the cached entry under that lock has to find the
    /// connection already retired, or it reuses one its owner has given up on.
    /// `retire` marks first and takes the lock second, so the mark is always
    /// there by then. Swap the order and the reader sees live every time.
    async fn retire_while_the_cache_lock_is_held(
        client: &MultiTargetGrpcClient<()>,
        target: &Uri,
        connection: &GrpcClientConnection<()>,
        cause: Status,
    ) {
        let held = client
            .connections
            .by_target
            .entry_async(target.clone())
            .await;
        let retiring = tokio::spawn({
            let (client, target, connection) = (client.clone(), target.clone(), connection.clone());
            async move { client.retire(&target, &connection, &cause).await }
        });

        expect(
            "retire had not marked the connection by the time it wanted the \
             lock, so a caller reading under that lock would see it as live",
            connection.retirement.retired.cancelled(),
        )
        .await;

        drop(held);
        retiring.await.unwrap();
    }

    #[test]
    async fn retire_marks_a_dead_connection_before_it_takes_the_cache_lock() {
        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_client();
        let cause = dead_transport_status().await;
        assert!(
            connection_is_gone(&cause),
            "the cause has to be one that means the connection is gone"
        );

        let connection = connect(&client, target.clone()).await.unwrap();
        retire_while_the_cache_lock_is_held(&client, &target, &connection, cause).await;

        assert!(
            connection.retirement.gone.is_cancelled(),
            "a connection that is gone left every request riding it parked on \
             it, each waiting its turn to re-dial"
        );
    }

    /// A status the peer sent back retires the connection too, and is the harder
    /// half of the ordering: nothing is released, so a guard reading only
    /// whether the connection is gone reads nothing at all.
    #[test]
    async fn retire_marks_a_connection_the_peer_gave_up_on_before_it_takes_the_cache_lock() {
        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_client();
        let cause = Status::unavailable("shard is being reassigned");
        assert!(
            !connection_is_gone(&cause),
            "the cause has to be one the peer sent back"
        );

        let connection = connect(&client, target.clone()).await.unwrap();
        retire_while_the_cache_lock_is_held(&client, &target, &connection, cause).await;

        assert!(
            !connection.retirement.gone.is_cancelled(),
            "a status the peer sent back cut short every request riding the \
             connection"
        );
    }

    /// Releasing a connection that is gone is what wakes every request parked
    /// on it, and that happens before the connection is cleared from the cache.
    /// So the whole cohort `retire` just released comes back for a connection
    /// while `retire` is still clearing, and the cache would hand each of them
    /// the same one it had already given up on.
    #[test]
    async fn a_connection_already_known_dead_is_not_handed_out_again() {
        let (target, _peer) = silent_peer().await;
        let (client, built) = counting_client();

        let first = connect(&client, target.clone()).await.unwrap();
        retired_by_the_peer(&first.retirement);

        let next = connect(&client, target.clone()).await.unwrap();
        assert_ne!(
            next.id, first.id,
            "the cache handed back a connection it had already given up on"
        );
        assert!(
            !next.retirement.is_retired(),
            "the connection put in its place came out retired too"
        );
        assert_eq!(
            built.load(Ordering::SeqCst),
            2,
            "the dead connection was replaced without a client being built for \
             the replacement"
        );
    }

    /// The two halves of a retirement reach the request riding the connection
    /// differently, and that is the whole reason there are two. A status the
    /// peer sent back leaves the request running: the transport works, and
    /// cutting it short would cost it a retry it never needed. A connection that
    /// is gone ends it at once, because waiting its turn to re-dial inside a
    /// dead `Channel` costs it a whole `connect_timeout` and everyone queued
    /// behind it another.
    #[test]
    async fn only_a_connection_that_is_gone_cuts_short_the_request_riding_it() {
        use std::task::Poll;

        fn never(_: &mut ()) -> Pin<Box<dyn Future<Output = Result<(), Status>> + Send + '_>> {
            Box::pin(std::future::pending())
        }

        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_client();
        let mut connection = connect(&client, target).await.unwrap();
        let retirement = connection.retirement.clone();

        let config = RetryConfig::default();
        let attempts = CallAttempts::new(&config, "test", "test", tracing::Span::none());
        let mut running = std::pin::pin!(attempt(&mut connection, &never, &attempts));
        assert!(
            futures::poll!(running.as_mut()).is_pending(),
            "the request ended before anything had happened to its connection"
        );

        retired_by_the_peer(&retirement);
        assert!(
            futures::poll!(running.as_mut()).is_pending(),
            "a status the peer sent back cut short a request the transport was \
             still carrying perfectly well"
        );

        retirement.retire(true);
        assert!(
            matches!(futures::poll!(running.as_mut()), Poll::Ready(Err(_))),
            "the request was left parked on a connection that is gone, waiting \
             its turn to re-dial"
        );
    }

    /// A call that failed on a dead connection has to retire it. Left cached it
    /// is handed to every caller after this one, and nothing takes it out,
    /// because taking it out is what this failure was for.
    #[test]
    async fn a_failed_call_retires_the_connection_it_rode() {
        let (target, _peer) = silent_peer().await;
        let client = MultiTargetGrpcClient::new("test", |_, _| (), one_attempt());
        let cause = Arc::new(dead_transport_status().await);

        let first = connect(&client, target.clone()).await.unwrap();
        let failed = client
            .call("test", target.clone(), move |_: &mut ()| {
                let cause = cause.clone();
                Box::pin(async move { Err::<(), Status>((*cause).clone()) })
            })
            .await;
        assert!(failed.is_err(), "the call under test has to have failed");

        assert!(
            first.retirement.is_retired(),
            "a call that failed on a dead connection left it fit to hand to \
             the next caller"
        );
        assert_ne!(
            connect(&client, target.clone()).await.unwrap().id,
            first.id,
            "the connection the failed call rode was handed out again"
        );
    }

    /// A retired attempt is replaced rather than inherited, and the replacement
    /// has to take its place in the map. Left out, the entry goes on answering
    /// with the retired one and every caller after this builds a connection of
    /// its own to that target, for good.
    #[test]
    async fn a_replacement_attempt_takes_the_place_of_the_one_it_replaces() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let (connections, build) = cache();

        let first = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        retired_by_the_peer(&first.retirement);

        let second = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_ne!(second.id, first.id, "a retired attempt was inherited");

        let third = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_eq!(
            third.id, second.id,
            "the replacement never took the retired one's place, so every \
             caller after it opens a connection of its own"
        );
    }

    /// A result that arrived in the same tick the connection was retired is a
    /// result, not a failure. Read as a failure it is retried, and an agent
    /// invocation that has already run runs again.
    #[test]
    async fn a_result_that_landed_wins_over_a_retirement_in_the_same_tick() {
        fn done(_: &mut ()) -> Pin<Box<dyn Future<Output = Result<(), Status>> + Send + '_>> {
            Box::pin(std::future::ready(Ok(())))
        }

        let config = RetryConfig::default();
        let attempts = CallAttempts::new(&config, "test", "test", tracing::Span::none());

        // Both arms are ready before the first poll, so which one is read first
        // is the whole of what this measures. Repeated because an unbiased
        // select would only sometimes read the wrong one.
        for _ in 0..100 {
            let mut connection = GrpcClientConnection {
                client: (),
                id: 0,
                retirement: Retirement::new(),
                last_used: LastUsed::now(),
            };
            connection.retirement.retire(true);

            assert!(
                attempt(&mut connection, &done, &attempts).await.is_ok(),
                "a result that had already arrived was thrown away for a \
                 retirement that landed at the same moment"
            );
        }
    }

    /// The twin of [`the_sweep_leaves_an_attempt_in_flight_alone`]. A failure
    /// naming an earlier connection must leave an attempt still being made: its
    /// own waiters would keep the one they hold, but every caller arriving after
    /// it starts a connection of its own, which is the cohort splintering in the
    /// middle of the reconnect that sharing exists to prevent.
    #[test]
    async fn forget_leaves_an_attempt_in_flight_alone() {
        let config = GrpcClientConfig::default();
        let (connections, _build) = cache();

        // Nothing listens here, and the connect is held open by a timeout far
        // longer than the test, so the attempt stays unsettled throughout.
        let target: Uri = "http://127.0.0.1:1/".parse().unwrap();
        let endpoint = build_endpoint(target.clone(), &config).unwrap();
        let attempt = attempt_bounded_by(endpoint, Duration::from_secs(600));
        connections
            .by_target
            .insert_async(target.clone(), attempt)
            .await
            .unwrap();

        connections.forget(&target, 0).await;

        assert!(
            connections.by_target.contains_async(&target).await,
            "a late failure took an attempt that callers were still waiting on"
        );
    }

    /// An attempt replacing one that was retired is driven like any other. Left
    /// undriven, losing its waiters parks it mid-connect with its deadline
    /// quietly expiring, and the next caller inherits an instant verdict about a
    /// connection nobody ever made. The arm that starts the first attempt to a
    /// target is not the same arm as the one that replaces it.
    #[test]
    async fn a_replacement_attempt_is_driven_like_the_first() {
        let config = GrpcClientConfig::default();
        let (connections, build) = cache();

        // Nothing listens here, so every attempt to it fails. Seeded rather than
        // made, because an attempt that failed on its own would have been
        // cleared by the very task this is about.
        let target: Uri = "http://127.0.0.1:1/".parse().unwrap();
        let endpoint = build_endpoint(target.clone(), &config).unwrap();
        let failed = attempt_bounded_by(endpoint, config.connect_timeout);
        assert!(
            failed.clone().await.is_err(),
            "the attempt being replaced has to be one nobody would inherit"
        );
        connections
            .by_target
            .insert_async(target.clone(), failed)
            .await
            .unwrap();

        assert!(
            connections
                .connect(target.clone(), &config, &build)
                .await
                .is_err(),
            "the replacement this test is about has to be one that failed"
        );

        expect(
            "a failed attempt replacing an earlier one was left in the map, so \
             nobody was finishing it and the next caller inherits its verdict",
            async {
                while connections.by_target.contains_async(&target).await {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            },
        )
        .await;
    }

    /// `drive` clears an attempt that failed, and a target can have a failed
    /// attempt and its replacement about at the same time. Keyed on the target
    /// alone, the failed attempt's own task would clear the replacement that had
    /// already taken its place.
    #[test]
    async fn a_failed_attempts_own_task_does_not_clear_the_replacement() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let (connections, build) = cache();

        // Nothing listens here, and no time at all to reach it.
        let endpoint = build_endpoint("http://127.0.0.1:1/".parse().unwrap(), &config).unwrap();
        let failed = attempt_bounded_by(endpoint, Duration::ZERO);
        assert!(
            failed.clone().await.is_err(),
            "the attempt this test is about has to be one that failed"
        );

        let replacement = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        connections.drive(target.clone(), failed).await.unwrap();

        let kept = connections
            .connect(target.clone(), &config, &build)
            .await
            .unwrap();
        assert_eq!(
            kept.id, replacement.id,
            "a failed attempt's own task cleared the replacement that had taken \
             its place"
        );
    }
}
